// Session-lifetime player seats and their carried cross-level state.
// See: context/lib/networking.md §Session-state ledger

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use postretro_entities::components::health::HealthComponent;
use postretro_entities::components::inventory::{Inventory, WIELDABLE_SLOT_CAPACITY};
use postretro_entities::components::weapon::WeaponComponent;
use postretro_entities::provenance::DescriptorProvenance;
#[cfg(test)]
use postretro_entities::provenance::DescriptorSpawnPath;
use postretro_entities::{AmmoReserve, EntityId, EntityRegistry};
use postretro_foundation::Seat;
use postretro_net::slots::SlotState;
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    ConnectClaim, RosterEntry, ServerControlMessage, SessionId, SessionRosterMessage,
};

const SEAT_NAMESPACE_SIZE: u32 = u16::MAX as u32 + 1;
/// Time a disconnected remote seat remains reclaimable by its asserted player
/// identity. The clock is driven once per render frame, including Frontend and
/// Loading frames where the fixed simulation does not run.
pub(crate) const HOLD_WINDOW: Duration = Duration::from_secs(30);

/// State retained by a session seat when its current pawn leaves a level.
///
/// A missing record deliberately seeds no defaults on a fresh seat.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CarriedState {
    pub(crate) health_current: Option<f32>,
    pub(crate) reserve: AmmoReserve,
    pub(crate) wieldables: [Option<String>; WIELDABLE_SLOT_CAPACITY],
    pub(crate) magazines: [Option<u32>; WIELDABLE_SLOT_CAPACITY],
    pub(crate) active_slot: usize,
}

impl Default for CarriedState {
    fn default() -> Self {
        Self {
            health_current: None,
            reserve: AmmoReserve::new(),
            wieldables: std::array::from_fn(|_| None),
            magazines: [None; WIELDABLE_SLOT_CAPACITY],
            active_slot: 0,
        }
    }
}

/// One exhaustive binding site for every carried field.
///
/// The `CarriedState` destructure has no wildcard: extending the state record
/// requires deciding whether the new value is harvested, restored, or skipped
/// at this ledger before the crate compiles.
enum CarriedField<'a> {
    HealthCurrent(&'a mut Option<f32>),
    Reserve(&'a mut AmmoReserve),
    Wieldables(&'a mut [Option<String>; WIELDABLE_SLOT_CAPACITY]),
    Magazines(&'a mut [Option<u32>; WIELDABLE_SLOT_CAPACITY]),
    ActiveSlot(&'a mut usize),
}

fn carried_fields(state: &mut CarriedState) -> [CarriedField<'_>; 5] {
    let CarriedState {
        health_current,
        reserve,
        wieldables,
        magazines,
        active_slot,
    } = state;
    [
        CarriedField::HealthCurrent(health_current),
        CarriedField::Reserve(reserve),
        CarriedField::Wieldables(wieldables),
        CarriedField::Magazines(magazines),
        CarriedField::ActiveSlot(active_slot),
    ]
}

/// Restore carried health after descriptor materialization.
///
/// Missing and nonpositive values keep the descriptor default so a fresh or
/// dead seat never materializes a dead pawn.
pub(crate) fn restore_carried_health(
    carried: Option<&CarriedState>,
    registry: &mut EntityRegistry,
    pawn: EntityId,
) {
    let Some(health_current) = carried
        .and_then(|state| state.health_current)
        .filter(|health| *health > 0.0)
    else {
        return;
    };
    postretro_entities::components::health::set_health_absolute(registry, pawn, health_current);
}

/// Future rejoin-hold expiry measured against the session's accumulated clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HoldDeadline(pub(crate) Duration);

/// Result of admitting one remote connection into the durable seat ledger.
///
/// A reclaim can repair an impossible-but-recoverable duplicate hold. The
/// caller owns mod-slot storage, so released losers travel with the admission
/// result instead of being hidden inside the seat table.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SeatAdmission {
    pub(crate) seat: Seat,
    pub(crate) released_seats: Vec<Seat>,
    /// Whether this connection reclaimed an in-hold seat whose per-owner values
    /// remain live and must not be overwritten by a persisted join seed.
    pub(crate) reclaimed: bool,
}

/// Durable host-local player identities for one running session.
///
/// The table is intentionally above transport participation: a `Seat` is minted
/// on admission and is never reused, while a client binding and a pawn binding
/// may disappear and later be replaced.
#[derive(Debug)]
pub(crate) struct SeatTable {
    session_id: SessionId,
    next_seat: u32,
    carried: HashMap<Seat, Option<CarriedState>>,
    client_bindings: HashMap<Seat, u64>,
    pawn_bindings: HashMap<Seat, EntityId>,
    /// Level-scoped seat placement. Kept separate from carried state so merely
    /// assigning a spawn cannot make an empty carry record authoritative.
    placement_assignments: HashMap<Seat, usize>,
    /// Placement provenance captured while level-spawned pawns are still at
    /// their authored origins. It remains valid after movement changes transforms.
    level_spawn_placements: HashMap<EntityId, usize>,
    connect_claims: HashMap<Seat, ConnectClaim>,
    hold_deadlines: HashMap<Seat, HoldDeadline>,
    /// Monotonic ordering is separate from the deadline: two disconnects can
    /// land in one poll at the same accumulated time, but reclaim must still
    /// pick the most recently held matching seat deterministically.
    hold_order: HashMap<Seat, u64>,
    next_hold_order: u64,
    hold_clock: Duration,
    next_placement_cursor: usize,
    /// Coalesces all roster-affecting mutations in one transport poll into one
    /// publication after its lifecycle batch completes.
    roster_dirty: bool,
}

impl SeatTable {
    /// Create the table for a single-player or listen-host session, reserving
    /// seat zero for the local player.
    pub(crate) fn new() -> Result<Self, getrandom::Error> {
        let mut session_id = [0; 16];
        getrandom::fill(&mut session_id)?;
        Ok(Self::with_session_id(SessionId(session_id)))
    }

    /// Create the local carry ledger used when session identity entropy is
    /// unavailable. Its sentinel id must never be published; the caller keeps
    /// networking disabled for this session.
    pub(crate) fn local_only() -> Self {
        Self::with_session_id(SessionId([0; 16]))
    }

    fn with_session_id(session_id: SessionId) -> Self {
        let mut carried = HashMap::new();
        carried.insert(Seat(0), None);
        Self {
            session_id,
            next_seat: 1,
            carried,
            client_bindings: HashMap::new(),
            pawn_bindings: HashMap::new(),
            placement_assignments: HashMap::new(),
            level_spawn_placements: HashMap::new(),
            connect_claims: HashMap::new(),
            hold_deadlines: HashMap::new(),
            hold_order: HashMap::new(),
            next_hold_order: 0,
            hold_clock: Duration::ZERO,
            next_placement_cursor: 0,
            roster_dirty: true,
        }
    }

    #[must_use]
    pub(crate) fn session_id(&self) -> SessionId {
        self.session_id
    }

    /// Whether this durable seat still belongs to the active session. Queued
    /// owner-slot reactions retain a copied `Seat`, so the app drain must check
    /// this before writing rather than recreating state for a released seat.
    pub(crate) fn contains_seat(&self, seat: Seat) -> bool {
        self.carried.contains_key(&seat)
    }

    /// Advance the session-relative hold clock once for this rendered frame.
    ///
    /// The caller is the sole frame-timing seam. Polling can happen more than
    /// once on a Splash or install-completion frame, so poll drains must never
    /// advance this clock themselves.
    pub(crate) fn advance_hold_clock(&mut self, frame_dt: Duration) {
        self.hold_clock = self.hold_clock.saturating_add(frame_dt);
    }

    /// Resolve the one admission chokepoint for a remote seat.
    ///
    /// A held seat is reclaimable only by exact equality of the opaque player
    /// id from the stored and incoming claims. A live holder is never displaced:
    /// a second connection asserting that id mints a fresh seat instead.
    pub(crate) fn admit_or_reclaim(
        &mut self,
        client_id: u64,
        claim: Option<ConnectClaim>,
        is_currently_closed: bool,
    ) -> Option<SeatAdmission> {
        if is_currently_closed {
            return None;
        }
        if let Some(seat) = self.seat_for_client(client_id) {
            return Some(SeatAdmission {
                seat,
                released_seats: Vec::new(),
                reclaimed: false,
            });
        }

        if let Some(incoming_claim) = claim.as_ref() {
            let held_matches: Vec<(Seat, u64)> = self
                .hold_deadlines
                .keys()
                .filter_map(|seat| {
                    self.hold_deadlines
                        .get(seat)
                        .filter(|deadline| deadline.0 >= self.hold_clock)?;
                    self.connect_claims
                        .get(seat)
                        .filter(|stored_claim| stored_claim.player_id == incoming_claim.player_id)
                        .map(|_| {
                            (
                                *seat,
                                *self
                                    .hold_order
                                    .get(seat)
                                    .expect("every held seat has a matching reclaim-order entry"),
                            )
                        })
                })
                .collect();

            if let Some((winner, _)) = held_matches.iter().max_by_key(|(_, order)| *order) {
                let winner = *winner;
                let mut released_seats = Vec::new();
                for (seat, _) in held_matches {
                    if seat != winner {
                        released_seats.push(self.release_seat(seat));
                    }
                }
                self.hold_deadlines.remove(&winner);
                self.hold_order.remove(&winner);
                self.client_bindings.insert(winner, client_id);
                // The claim is host-local. Retain the rejoining connection's
                // complete assertion while matching only its opaque player id.
                self.connect_claims
                    .insert(winner, claim.expect("claim borrowed above"));
                self.roster_dirty = true;
                return Some(SeatAdmission {
                    seat: winner,
                    released_seats,
                    reclaimed: true,
                });
            }

            let has_live_identity_collision = self.client_bindings.keys().any(|seat| {
                self.connect_claims
                    .get(seat)
                    .is_some_and(|stored_claim| stored_claim.player_id == incoming_claim.player_id)
            });
            if has_live_identity_collision {
                log::warn!(
                    "[Net] client {client_id} asserted a player identity already held by a live connection; minting a fresh seat"
                );
            }
        } else {
            log::warn!(
                "[Net] admitted client {client_id} has no valid player identity claim; minting an anonymous unreclaimable seat"
            );
        }

        self.mint_fresh_seat(client_id, claim)
    }

    /// Mint a non-reusable remote seat after admission chose not to reclaim.
    ///
    /// `None` means the u16 seat namespace is exhausted; the caller keeps the
    /// connection live, but it cannot acquire a durable player seat.
    fn mint_fresh_seat(
        &mut self,
        client_id: u64,
        claim: Option<ConnectClaim>,
    ) -> Option<SeatAdmission> {
        let Some(seat_number) = u16::try_from(self.next_seat).ok() else {
            // The admitted client still needs one status roster identifying
            // `your_seat: None` even though no row can be allocated for it.
            self.roster_dirty = true;
            return None;
        };
        let seat = Seat(seat_number);
        let Some(next_seat) = self.next_seat.checked_add(1) else {
            self.roster_dirty = true;
            return None;
        };
        self.next_seat = next_seat;
        self.carried.insert(seat, None);
        self.client_bindings.insert(seat, client_id);
        if let Some(claim) = claim {
            self.connect_claims.insert(seat, claim);
        }
        self.roster_dirty = true;
        Some(SeatAdmission {
            seat,
            released_seats: Vec::new(),
            reclaimed: false,
        })
    }

    #[must_use]
    pub(crate) fn seat_for_client(&self, client_id: u64) -> Option<Seat> {
        self.client_bindings
            .iter()
            .find_map(|(seat, bound)| (*bound == client_id).then_some(*seat))
    }

    /// The one write path for the pawn/seat relationship.
    ///
    /// The seat-keyed map here and the registry's pawn-keyed reverse index are
    /// two views of one fact, so they are written together and nowhere else:
    /// every other mention of `pawn_bindings` is a read. `None` unbinds. A
    /// rebind clears the outgoing pawn's entry first, because the reverse index
    /// is many-to-one and an overwrite alone would leave two pawns resolving to
    /// the same seat.
    fn set_pawn_binding(
        &mut self,
        registry: &mut EntityRegistry,
        seat: Seat,
        pawn: Option<EntityId>,
    ) {
        if let Some(previous) = self.pawn_bindings.remove(&seat) {
            registry.clear_pawn_seat(previous);
        }
        if let Some(pawn) = pawn {
            self.pawn_bindings.insert(seat, pawn);
            registry.bind_pawn_seat(pawn, seat);
        }
    }

    /// Resolve the durable pawn binding while its connection is still live.
    ///
    /// Level-parity teardown can retire the endpoint's level-scoped slot map
    /// before a transport disconnect is observed. The seat binding survives
    /// long enough to provide the final harvest and despawn route.
    #[must_use]
    pub(crate) fn pawn_for_client(&self, client_id: u64) -> Option<EntityId> {
        let seat = self.seat_for_client(client_id)?;
        self.pawn_bindings.get(&seat).copied()
    }

    /// Start a reclaim hold after the transport reports a slot disconnect.
    ///
    /// This deliberately does not harvest: lifecycle cleanup owns pawn
    /// destruction and harvests immediately before it. A drop while demoted or
    /// Loading can have no lifecycle event, but still reaches this transport edge.
    pub(crate) fn hold_disconnected_client(
        &mut self,
        registry: &mut EntityRegistry,
        client_id: u64,
    ) -> Option<Seat> {
        let seat = self.seat_for_client(client_id)?;
        self.client_bindings.remove(&seat);
        // The caller resolves and tears down this binding before starting the
        // hold. Retaining it after that edge can only leave a stale pawn id;
        // the carried record retains the durable rejoin state instead. A held
        // seat has no live owner, so a pawn that outlives the hold edge must
        // resolve to nothing rather than to an unoccupied seat.
        self.set_pawn_binding(registry, seat, None);
        let deadline = self.hold_clock.saturating_add(HOLD_WINDOW);
        self.hold_deadlines.insert(seat, HoldDeadline(deadline));
        let order = self.next_hold_order;
        self.next_hold_order = self.next_hold_order.wrapping_add(1);
        self.hold_order.insert(seat, order);
        self.roster_dirty = true;
        Some(seat)
    }

    /// Release every hold whose deadline is reached. Seats remain monotonic:
    /// release removes their host-local state and roster entry, but never moves
    /// `next_seat` backward.
    pub(crate) fn release_expired_holds(&mut self) -> Vec<Seat> {
        let expired: Vec<Seat> = self
            .hold_deadlines
            .iter()
            .filter_map(|(seat, deadline)| (deadline.0 <= self.hold_clock).then_some(*seat))
            .collect();
        expired
            .into_iter()
            .map(|seat| self.release_seat(seat))
            .collect()
    }

    fn release_seat(&mut self, seat: Seat) -> Seat {
        debug_assert_ne!(seat, Seat(0), "the local seat is never held or released");
        self.carried.remove(&seat);
        self.client_bindings.remove(&seat);
        // A seat only reaches release from a hold, and `hold_disconnected_client`
        // unbinds the pawn on that edge. Asserting instead of removing keeps
        // `set_pawn_binding` the sole write path — release has no registry.
        debug_assert!(
            !self.pawn_bindings.contains_key(&seat),
            "a held seat has already had its pawn binding cleared"
        );
        self.placement_assignments.remove(&seat);
        self.connect_claims.remove(&seat);
        self.hold_deadlines.remove(&seat);
        self.hold_order.remove(&seat);
        self.roster_dirty = true;
        seat
    }

    /// Whether this poll needs one consolidated roster publication.
    pub(crate) fn take_roster_dirty(&mut self) -> bool {
        std::mem::take(&mut self.roster_dirty)
    }

    /// Join capacity remaining after connected and held remote seats reserve
    /// their places. The monotonic seat namespace is a separate hard limit.
    #[must_use]
    pub(crate) fn open_seat_count(&self) -> u32 {
        if self.next_seat >= SEAT_NAMESPACE_SIZE {
            return 0;
        }
        let retained_remote_seats = self.carried.len().saturating_sub(1);
        u32::try_from(super::MAX_CLIENTS)
            .expect("configured client capacity fits wire count")
            .saturating_sub(u32::try_from(retained_remote_seats).unwrap_or(u32::MAX))
    }

    /// Build a deterministic seat-keyed roster snapshot from the table's own
    /// lifecycle bindings. Claims and carried contents never leave this table:
    /// only the host-minted seat and current connection fact are projected.
    #[must_use]
    pub(crate) fn roster_entries(&self) -> Vec<RosterEntry> {
        let mut seats: Vec<Seat> = self.carried.keys().copied().collect();
        seats.sort_unstable_by_key(|seat| seat.0);
        seats
            .into_iter()
            .map(|seat| RosterEntry {
                seat: seat.0,
                connected: seat == Seat(0) || self.client_bindings.contains_key(&seat),
            })
            .collect()
    }

    fn roster_message_for(&self, client_id: u64) -> SessionRosterMessage {
        SessionRosterMessage {
            session_id: self.session_id(),
            your_seat: self.seat_for_client(client_id).map(|seat| seat.0),
            open_seats: self.open_seat_count(),
            entries: self.roster_entries(),
        }
    }

    /// Associate a newly spawned pawn with its durable seat.
    ///
    /// A seat this table never minted binds nothing at all — the reverse index
    /// stays in lockstep with that no-op rather than gaining an owner the seat
    /// ledger does not know about.
    pub(crate) fn bind_pawn(&mut self, registry: &mut EntityRegistry, seat: Seat, pawn: EntityId) {
        if self.carried.contains_key(&seat) {
            self.set_pawn_binding(registry, seat, Some(pawn));
            if let Some(placement) = self.level_spawn_placements.get(&pawn).copied() {
                self.placement_assignments.insert(seat, placement);
            }
        }
    }

    /// Capture one level-spawned pawn's authored placement before simulation
    /// can move it away from that origin.
    pub(crate) fn bind_level_spawn_placement(&mut self, pawn: EntityId, placement: usize) {
        self.level_spawn_placements.insert(pawn, placement);
    }

    /// Preserve every carryable component currently present on this pawn.
    ///
    /// Lookup is by pawn identity, not current client binding: a same-poll
    /// disconnect/rebind cannot make the old dying pawn write into the new one.
    pub(crate) fn harvest_pawn(&mut self, registry: &EntityRegistry, pawn: EntityId) {
        let Some(seat) = registry.seat_for_pawn(pawn) else {
            return;
        };
        let health_current = registry
            .get_component::<HealthComponent>(pawn)
            .ok()
            .map(|health| health.current);
        let reserve = registry.get_component::<AmmoReserve>(pawn).ok().cloned();
        let inventory = registry.get_component::<Inventory>(pawn).ok().cloned();
        let harvested_weapons: Option<[Option<(String, u32)>; WIELDABLE_SLOT_CAPACITY]> =
            inventory.as_ref().map(|inventory| {
                std::array::from_fn(|slot| {
                    let weapon = inventory.wieldables[slot]?;
                    let canonical_name = registry
                        .get_component::<DescriptorProvenance>(weapon)
                        .ok()?
                        .canonical_name
                        .clone();
                    let magazine = registry
                        .get_component::<WeaponComponent>(weapon)
                        .ok()?
                        .magazine;
                    Some((canonical_name, magazine))
                })
            });
        if health_current.is_none() && reserve.is_none() && inventory.is_none() {
            return;
        }
        let carried = self
            .carried
            .entry(seat)
            .or_insert_with(|| Some(CarriedState::default()));
        let state = carried.get_or_insert_with(CarriedState::default);

        for field in carried_fields(state) {
            match field {
                CarriedField::HealthCurrent(carried_health) => {
                    if let Some(health_current) = health_current {
                        *carried_health = Some(health_current);
                    }
                }
                CarriedField::Reserve(carried_reserve) => {
                    if let Some(reserve) = reserve.as_ref() {
                        *carried_reserve = reserve.clone();
                    }
                }
                CarriedField::Wieldables(carried_wieldables) => {
                    if let Some(harvested_weapons) = harvested_weapons.as_ref() {
                        for (slot, weapon) in harvested_weapons.iter().enumerate() {
                            carried_wieldables[slot] = weapon
                                .as_ref()
                                .map(|(canonical_name, _)| canonical_name.clone());
                        }
                    }
                }
                CarriedField::Magazines(carried_magazines) => {
                    if let Some(harvested_weapons) = harvested_weapons.as_ref() {
                        for (slot, weapon) in harvested_weapons.iter().enumerate() {
                            // A canonical descriptor name and its magazine are
                            // one carried weapon record. A missing component
                            // clears both rather than combining a stale name
                            // with a freshly-read magazine.
                            carried_magazines[slot] =
                                weapon.as_ref().map(|(_, magazine)| *magazine);
                        }
                    }
                }
                CarriedField::ActiveSlot(carried_active_slot) => {
                    if let Some(inventory) = inventory.as_ref() {
                        *carried_active_slot = inventory.active_slot;
                    }
                }
            }
        }
    }

    /// Harvest every currently bound pawn. Missing pawns/components preserve
    /// their prior records exactly.
    pub(crate) fn harvest_bound_pawns(&mut self, registry: &EntityRegistry) {
        let pawns: Vec<EntityId> = self.pawn_bindings.values().copied().collect();
        for pawn in pawns {
            self.harvest_pawn(registry, pawn);
        }
    }

    #[must_use]
    pub(crate) fn carried_state(&self, seat: Seat) -> Option<&CarriedState> {
        self.carried.get(&seat).and_then(Option::as_ref)
    }

    /// Reset live-pawn identity and level-scoped placement after actual level
    /// unload. Suspension does not call this: its world and pawn bindings remain
    /// live on resume.
    ///
    /// The registry's reverse index is already empty by the time this runs:
    /// `EntityRegistry::clear_for_level_unload` despawns every live entity, and
    /// `despawn` drops the pawn's seat entry. Unbinding through the one write
    /// path anyway covers the reverse order and any seat whose pawn was never
    /// despawned — entity indices are recycled, so a surviving entry could later
    /// be matched by an unrelated entity.
    pub(crate) fn clear_pawn_bindings_for_level_unload(&mut self, registry: &mut EntityRegistry) {
        for seat in self.pawn_bindings.keys().copied().collect::<Vec<_>>() {
            self.set_pawn_binding(registry, seat, None);
        }
        self.placement_assignments.clear();
        self.level_spawn_placements.clear();
    }

    /// Resolve live placement occupancy from durable pawn associations, not
    /// from positions that movement changes on the first simulation tick.
    pub(crate) fn occupied_live_placements(
        &self,
        registry: &EntityRegistry,
        placement_count: usize,
    ) -> HashSet<usize> {
        let mut occupied = HashSet::new();
        for (seat, pawn) in &self.pawn_bindings {
            if registry.exists(*pawn)
                && let Some(placement) = self.placement_assignments.get(seat)
                && *placement < placement_count
            {
                occupied.insert(*placement);
            }
        }
        for (pawn, placement) in &self.level_spawn_placements {
            if registry.exists(*pawn) && *placement < placement_count {
                occupied.insert(*placement);
            }
        }
        occupied
    }

    /// Assign or recall a placement for `seat` without allowing a held seat or a
    /// currently-live pawn to be overlapped whenever a free index exists.
    ///
    /// `live_placements` deliberately comes from the spawning path rather than
    /// from seat bindings alone: map installation may leave live player pawns
    /// that are not currently represented by a remote connection binding.
    pub(crate) fn assign_placement(
        &mut self,
        seat: Seat,
        placement_count: usize,
        live_placements: impl IntoIterator<Item = usize>,
    ) -> Option<usize> {
        if placement_count == 0 || !self.carried.contains_key(&seat) {
            return None;
        }

        if let Some(placement) = self
            .placement_assignments
            .get(&seat)
            .copied()
            .filter(|placement| *placement < placement_count)
        {
            return Some(placement);
        }

        let mut occupied: HashSet<usize> = live_placements
            .into_iter()
            .filter(|placement| *placement < placement_count)
            .collect();
        occupied.extend(
            self.placement_assignments
                .iter()
                .filter(|(other, _)| **other != seat && !self.client_bindings.contains_key(other))
                .map(|(_, placement)| *placement)
                .filter(|placement| *placement < placement_count),
        );

        let fallback = self.next_placement_cursor % placement_count;
        let placement = (0..placement_count)
            .map(|offset| (self.next_placement_cursor + offset) % placement_count)
            .find(|candidate| !occupied.contains(candidate))
            .unwrap_or_else(|| {
                log::warn!(
                    "[Net] every player_spawn placement is occupied; reusing index {fallback}"
                );
                fallback
            });
        self.next_placement_cursor = placement.wrapping_add(1);
        self.placement_assignments.insert(seat, placement);
        Some(placement)
    }

    #[cfg(test)]
    fn carried_state_for_test(&self, seat: Seat) -> Option<&CarriedState> {
        self.carried_state(seat)
    }

    #[cfg(test)]
    pub(crate) fn from_test_session_id(session_id: [u8; 16]) -> Self {
        Self::with_session_id(SessionId(session_id))
    }
}

fn is_roster_recipient(slot_state: Option<SlotState>) -> bool {
    matches!(
        slot_state,
        Some(SlotState::Admitted) | Some(SlotState::Participating)
    )
}

/// Publish one coalesced roster revision after an ordered transport lifecycle
/// drain. Pending and closed peers receive no frame at all; each admitted or
/// participating recipient gets a separately encoded `your_seat` projection.
pub(crate) fn publish_dirty_roster(server: &mut NetServer, seats: &mut SeatTable) {
    if !seats.take_roster_dirty() {
        return;
    }

    for client_id in server.connected_clients() {
        if !is_roster_recipient(server.slot_state(client_id)) {
            continue;
        }
        let roster = seats.roster_message_for(client_id);
        server.send_control(
            client_id,
            postretro_net::wire::encode(&ServerControlMessage::SessionRoster(roster)),
        );
    }
}

/// Finish one drained host poll after disconnect, admission, and lifecycle work.
///
/// Reclaim is resolved by the admission batch before this function runs, so a
/// connection arriving on its deadline frame keeps its held seat. Expiry and
/// roster publication then happen together at this sole post-drain seam.
pub(crate) fn finish_host_poll(server: &mut NetServer, seats: &mut SeatTable) -> Vec<Seat> {
    let released_seats = seats.release_expired_holds();
    publish_dirty_roster(server, seats);
    released_seats
}

#[cfg(test)]
mod roster_harness_test;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr, UdpSocket};

    use postretro_entities::components::health::HealthComponent;
    use postretro_entities::data_descriptors::HealthDescriptor;
    use postretro_entities::registry::Transform;
    use postretro_entities::{
        ReplicationScope, SlotOwnership, SlotRecord, SlotSchema, SlotTable as StateSlotTable,
        SlotType, SlotValue,
    };
    use postretro_net::slots::{CloseCause, SlotEvent, SlotState, SlotTable};
    use postretro_net::transport::{HandshakeOutcome, NetClient};
    use postretro_net::wire::encode_connect_claim;
    use postretro_scripting_core::data_descriptors::{FireMode, ResolutionMode, WeaponDescriptor};
    use postretro_test_log_capture::LogCapture;

    fn health(max: f32, current: f32) -> HealthComponent {
        let mut health = HealthComponent::from_descriptor(&HealthDescriptor {
            max,
            hitbox: None,
            zone_multipliers: HashMap::new(),
        });
        health.current = current;
        health
    }

    fn assert_health_eq(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "expected {expected}, got {actual}"
        );
    }

    fn per_seat_number_slot(default: f32) -> SlotRecord {
        SlotRecord::new(SlotSchema {
            slot_type: SlotType::Number,
            default: Some(SlotValue::Number(default)),
            range: None,
            persist: false,
            readonly: false,
            ownership: SlotOwnership::Mod,
            network: ReplicationScope::None,
            per_owner: false,
            accumulate: None,
        })
    }

    fn weapon(magazine: u32) -> WeaponComponent {
        let mut weapon = WeaponComponent::from_descriptor(&WeaponDescriptor {
            damage: 10.0,
            pellet_count: 1,
            spread_degrees: 0.0,
            range: 20.0,
            cooldown_ms: 100.0,
            fire_mode: FireMode::Semi,
            resolution: ResolutionMode::Hitscan,
            projectile: None,
            credit_source: None,
            third_person_model: None,
            viewmodel: None,
            placement: None,
            muzzle_offset: None,
            resource: None,
            lower_ms: 0,
            raise_ms: 0,
            block_during_reload: None,
        });
        weapon.magazine = magazine;
        weapon
    }

    fn provenance(canonical_name: &str) -> DescriptorProvenance {
        DescriptorProvenance {
            canonical_name: canonical_name.to_string(),
            owned_components: Default::default(),
            map_overrides: Default::default(),
            spawn_path: DescriptorSpawnPath::DefaultWeapon,
        }
    }

    fn claim(id: [u8; 16], display_name: &str) -> ConnectClaim {
        ConnectClaim {
            player_id: postretro_net::wire::PlayerClaimId(id),
            display_name: display_name.to_owned(),
        }
    }

    const RELAY_STEP: Duration = Duration::from_millis(16);
    const RELAY_MOD_DIGEST: [u8; 32] = [0x41; 32];
    const RELAY_LEVEL_DIGEST: [u8; 32] = [0x53; 32];

    /// Construct a claimed relay connection with the real admission and parity
    /// declarations. The relay is deliberately only a transport stand-in: seat
    /// ownership remains in the engine-side table below it.
    fn relay_pair(
        client_id: u64,
        claim_data: Option<[u8; postretro_net::wire::NETCODE_USER_DATA_BYTES]>,
    ) -> (NetServer, NetClient, SocketAddr) {
        let server_socket =
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("fixture server binds loopback");
        let server_addr: SocketAddr = server_socket
            .local_addr()
            .expect("fixture server resolves loopback address");
        let mut server =
            NetServer::new(server_socket, server_addr, 4, Duration::from_secs(1), None)
                .expect("fixture server transport constructs");
        let mut client = relay_client(server_addr, client_id);

        server.add_relay_connection(client_id, claim_data);
        configure_relay_parity(&mut server, &mut client, "seat-fixture-level");
        (server, client, server_addr)
    }

    fn relay_client(server_addr: SocketAddr, client_id: u64) -> NetClient {
        let client_socket =
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("fixture client binds loopback");
        let mut client = NetClient::new(
            client_socket,
            server_addr,
            client_id,
            Duration::from_secs(1),
            None,
            None,
        )
        .expect("fixture client transport constructs");
        client.set_connected();
        client
    }

    fn configure_relay_parity(server: &mut NetServer, client: &mut NetClient, level: &str) {
        server.set_mod_identity("postretro.seat-test".to_string(), "1.0.0".to_string());
        server.set_mod_digest(Some(RELAY_MOD_DIGEST));
        server.set_level_parity(Some((level.to_string(), RELAY_LEVEL_DIGEST)));
        client.set_mod_identity("postretro.seat-test".to_string(), "1.0.0".to_string());
        client.set_mod_digest(Some(RELAY_MOD_DIGEST));
        client.set_level_parity(Some((level.to_string(), RELAY_LEVEL_DIGEST)));
    }

    fn relay_client_to_server(client_id: u64, client: &mut NetClient, server: &mut NetServer) {
        client.update_connections(RELAY_STEP);
        for packet in client.packets_to_send() {
            server.process_packet_from(&packet, client_id);
        }
    }

    fn admit_relay_client(
        client_id: u64,
        client: &mut NetClient,
        server: &mut NetServer,
        seats: &mut SeatTable,
    ) -> Seat {
        relay_client_to_server(client_id, client, server);
        let poll = server.poll_handshakes();
        assert!(
            poll.disconnects.is_empty(),
            "an admission fixture must not disconnect its relay client"
        );
        let admitted_client = poll.handshakes.iter().find_map(|outcome| match outcome {
            HandshakeOutcome::Admitted { client_id } => Some(*client_id),
            HandshakeOutcome::Rejected { .. } | HandshakeOutcome::ParityHeld { .. } => None,
        });
        assert_eq!(admitted_client, Some(client_id));
        let seat = seats
            .admit_or_reclaim(
                client_id,
                server.connect_claim(client_id).cloned(),
                server.is_closed(client_id),
            )
            .expect("the fixture seat namespace has room")
            .seat;
        finish_host_poll(server, seats);
        seat
    }

    fn hold_closed_relay_client(
        client_id: u64,
        server: &mut NetServer,
        seats: &mut SeatTable,
        registry: &mut EntityRegistry,
    ) {
        assert!(matches!(
            server.close_relay_connection(client_id, CloseCause::Disconnect),
            Some(SlotEvent::Closed {
                client_id: closed,
                cause: CloseCause::Disconnect,
            }) if closed == client_id
        ));
        let poll = server.poll_handshakes();
        assert_eq!(poll.disconnects, vec![client_id]);
        assert_eq!(
            seats.hold_disconnected_client(registry, client_id),
            Some(Seat(1))
        );
        finish_host_poll(server, seats);
    }

    #[test]
    fn relay_claim_rejoins_with_its_seat_and_carried_state_after_a_level_change() {
        const FIRST_CLIENT: u64 = 41;
        const REJOINED_CLIENT: u64 = 42;
        let asserted_claim = claim([0x42; 16], "Relay Runner");
        let (mut server, mut client, server_addr) =
            relay_pair(FIRST_CLIENT, Some(encode_connect_claim(&asserted_claim)));
        let mut seats = SeatTable::from_test_session_id([0x99; 16]);

        assert_eq!(server.connect_claim(FIRST_CLIENT), Some(&asserted_claim));
        let original_seat = admit_relay_client(FIRST_CLIENT, &mut client, &mut server, &mut seats);
        assert_eq!(original_seat, Seat(1));

        let mut old_level = EntityRegistry::new();
        let old_pawn = old_level.spawn(Transform::default());
        old_level
            .set_component(old_pawn, health(100.0, 37.0))
            .expect("fixture pawn accepts health");
        let mut reserve = AmmoReserve::new();
        reserve.credit("shells", 19);
        old_level
            .set_component(old_pawn, reserve)
            .expect("fixture pawn accepts reserve");
        seats.bind_pawn(&mut old_level, original_seat, old_pawn);

        // A host level change only demotes the connection. Its seat must survive
        // while the old pawn is harvested and the world retires its entity ids.
        server.set_level_parity(None);
        let demotion = server.poll_handshakes();
        assert!(matches!(
            demotion.lifecycle.as_slice(),
            [SlotEvent::Demoted {
                client_id: FIRST_CLIENT,
                ..
            }]
        ));
        seats.harvest_bound_pawns(&old_level);
        old_level.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut old_level);
        let carried = seats
            .carried_state(original_seat)
            .expect("level harvest leaves a carried record");
        assert_health_eq(carried.health_current.expect("health carries"), 37.0);
        assert_eq!(carried.reserve.available("shells"), 19);
        assert_eq!(seats.seat_for_client(FIRST_CLIENT), Some(original_seat));

        configure_relay_parity(&mut server, &mut client, "seat-fixture-next-level");
        relay_client_to_server(FIRST_CLIENT, &mut client, &mut server);
        let repromotion = server.poll_handshakes();
        assert!(matches!(
            repromotion.lifecycle.as_slice(),
            [SlotEvent::Participating {
                client_id: FIRST_CLIENT,
            }]
        ));
        assert_eq!(seats.seat_for_client(FIRST_CLIENT), Some(original_seat));
        finish_host_poll(&mut server, &mut seats);

        hold_closed_relay_client(FIRST_CLIENT, &mut server, &mut seats, &mut old_level);
        assert_eq!(seats.seat_for_client(FIRST_CLIENT), None);
        assert!(
            seats.carried_state(original_seat).is_some(),
            "the disconnected seat holds its carry until reclaim or expiry"
        );

        let mut rejoined_client = relay_client(server_addr, REJOINED_CLIENT);
        server.add_relay_connection(REJOINED_CLIENT, Some(encode_connect_claim(&asserted_claim)));
        configure_relay_parity(&mut server, &mut rejoined_client, "seat-fixture-next-level");
        let reclaimed = admit_relay_client(
            REJOINED_CLIENT,
            &mut rejoined_client,
            &mut server,
            &mut seats,
        );

        assert_eq!(reclaimed, original_seat);
        assert_eq!(seats.seat_for_client(REJOINED_CLIENT), Some(original_seat));
        let restored = seats
            .carried_state(reclaimed)
            .expect("reclaimed seat keeps its harvested carry");
        assert_health_eq(
            restored.health_current.expect("health remains carried"),
            37.0,
        );
        assert_eq!(restored.reserve.available("shells"), 19);
    }

    #[test]
    fn relay_rejoin_after_injected_hold_expiry_mints_fresh_default_seat() {
        const FIRST_CLIENT: u64 = 51;
        const REJOINED_CLIENT: u64 = 52;
        let asserted_claim = claim([0x52; 16], "Expiry Runner");
        let (mut server, mut client, server_addr) =
            relay_pair(FIRST_CLIENT, Some(encode_connect_claim(&asserted_claim)));
        let mut seats = SeatTable::from_test_session_id([0x52; 16]);
        let mut registry = EntityRegistry::new();
        let expired_seat = admit_relay_client(FIRST_CLIENT, &mut client, &mut server, &mut seats);
        seats.carried.insert(
            expired_seat,
            Some(CarriedState {
                health_current: Some(18.0),
                ..Default::default()
            }),
        );

        hold_closed_relay_client(FIRST_CLIENT, &mut server, &mut seats, &mut registry);
        seats.advance_hold_clock(HOLD_WINDOW);
        finish_host_poll(&mut server, &mut seats);
        assert_eq!(seats.carried_state(expired_seat), None);

        let mut rejoined_client = relay_client(server_addr, REJOINED_CLIENT);
        server.add_relay_connection(REJOINED_CLIENT, Some(encode_connect_claim(&asserted_claim)));
        configure_relay_parity(&mut server, &mut rejoined_client, "seat-fixture-level");
        let fresh = admit_relay_client(
            REJOINED_CLIENT,
            &mut rejoined_client,
            &mut server,
            &mut seats,
        );

        assert_eq!(fresh, Seat(2), "expired seat numbers are never reused");
        assert!(
            seats.carried_state(fresh).is_none(),
            "a rejoin after expiry starts from descriptor defaults"
        );
    }

    #[test]
    fn relay_absent_or_corrupt_claim_mints_anonymous_fresh_seat() {
        for (client_id, user_data) in [
            (61, None),
            (
                62,
                Some([0xff; postretro_net::wire::NETCODE_USER_DATA_BYTES]),
            ),
        ] {
            let (mut server, mut client, _) = relay_pair(client_id, user_data);
            let mut seats = SeatTable::from_test_session_id([client_id as u8; 16]);

            assert_eq!(
                server.connect_claim(client_id),
                None,
                "an absent or corrupt envelope must never create a reclaimable claim"
            );
            let seat = admit_relay_client(client_id, &mut client, &mut server, &mut seats);
            assert_eq!(seat, Seat(1));
            assert!(
                seats.carried_state(seat).is_none(),
                "an anonymous admission begins with descriptor defaults"
            );
        }
    }

    #[test]
    fn relay_disconnects_while_demoted_or_never_promoted_still_start_a_hold() {
        const DEMOTED_CLIENT: u64 = 63;
        let demoted_claim = claim([0x63; 16], "Loading Runner");
        let (mut demoted_server, mut demoted_client, _) =
            relay_pair(DEMOTED_CLIENT, Some(encode_connect_claim(&demoted_claim)));
        let mut demoted_seats = SeatTable::from_test_session_id([0x63; 16]);
        let mut registry = EntityRegistry::new();
        let demoted_seat = admit_relay_client(
            DEMOTED_CLIENT,
            &mut demoted_client,
            &mut demoted_server,
            &mut demoted_seats,
        );
        demoted_server.set_level_parity(None);
        assert!(matches!(
            demoted_server.poll_handshakes().lifecycle.as_slice(),
            [SlotEvent::Demoted {
                client_id: DEMOTED_CLIENT,
                ..
            }]
        ));
        assert_eq!(
            demoted_server.close_relay_connection(DEMOTED_CLIENT, CloseCause::Disconnect),
            None,
            "a demoted slot has no closed lifecycle event to key the hold from"
        );
        assert_eq!(
            demoted_server.poll_handshakes().disconnects,
            vec![DEMOTED_CLIENT]
        );
        assert_eq!(
            demoted_seats.hold_disconnected_client(&mut registry, DEMOTED_CLIENT),
            Some(demoted_seat)
        );

        const ADMITTED_CLIENT: u64 = 64;
        let admitted_claim = claim([0x64; 16], "Waiting Runner");
        let (mut admitted_server, mut admitted_client, _) =
            relay_pair(ADMITTED_CLIENT, Some(encode_connect_claim(&admitted_claim)));
        admitted_server.set_level_parity(None);
        admitted_client.set_level_parity(None);
        let mut admitted_seats = SeatTable::from_test_session_id([0x64; 16]);
        let admitted_seat = admit_relay_client(
            ADMITTED_CLIENT,
            &mut admitted_client,
            &mut admitted_server,
            &mut admitted_seats,
        );
        assert_eq!(
            admitted_server.slot_state(ADMITTED_CLIENT),
            Some(SlotState::Admitted)
        );
        assert_eq!(
            admitted_server.close_relay_connection(ADMITTED_CLIENT, CloseCause::Disconnect),
            None,
            "an admitted peer that never promoted likewise emits no closed lifecycle event"
        );
        assert_eq!(
            admitted_server.poll_handshakes().disconnects,
            vec![ADMITTED_CLIENT]
        );
        assert_eq!(
            admitted_seats.hold_disconnected_client(&mut registry, ADMITTED_CLIENT),
            Some(admitted_seat)
        );
    }

    #[test]
    fn reclaim_with_a_fresh_client_id_never_reopens_the_closed_transport_slot() {
        const CLOSED_CLIENT: u64 = 71;
        const REJOINED_CLIENT: u64 = 72;
        let player_claim = claim([0x71; 16], "Terminal Runner");
        let mut seats = SeatTable::from_test_session_id([0x71; 16]);
        let mut registry = EntityRegistry::new();
        let original_seat = seats
            .admit_or_reclaim(CLOSED_CLIENT, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        assert_eq!(
            seats.hold_disconnected_client(&mut registry, CLOSED_CLIENT),
            Some(original_seat)
        );

        let mut slots = SlotTable::new();
        slots.on_connect(CLOSED_CLIENT);
        let _ = slots.admit(CLOSED_CLIENT);
        assert_eq!(slots.close(CLOSED_CLIENT, CloseCause::Disconnect), None);
        assert_eq!(
            slots.state(CLOSED_CLIENT),
            Some(SlotState::Closed {
                cause: CloseCause::Disconnect,
            })
        );

        slots.on_connect(REJOINED_CLIENT);
        assert_eq!(slots.state(REJOINED_CLIENT), Some(SlotState::Pending));
        let reclaimed = seats
            .admit_or_reclaim(REJOINED_CLIENT, Some(player_claim), false)
            .expect("fresh transport id reclaims the held seat")
            .seat;
        let _ = slots.admit(REJOINED_CLIENT);

        assert_eq!(reclaimed, original_seat);
        assert_eq!(slots.state(REJOINED_CLIENT), Some(SlotState::Admitted));
        assert_eq!(
            slots.state(CLOSED_CLIENT),
            Some(SlotState::Closed {
                cause: CloseCause::Disconnect,
            }),
            "reclaim binds a new client id instead of touching the terminal slot"
        );
        assert_eq!(
            slots.participate(CLOSED_CLIENT),
            None,
            "the previously closed id remains terminal"
        );
    }

    #[test]
    fn single_player_health_survives_level_boundary() {
        let mut seats = SeatTable::from_test_session_id([7; 16]);
        let mut old_registry = EntityRegistry::new();
        let old_pawn = old_registry.spawn(Transform::default());
        old_registry
            .set_component(old_pawn, health(100.0, 37.5))
            .unwrap();
        seats.bind_pawn(&mut old_registry, Seat(0), old_pawn);

        seats.harvest_bound_pawns(&old_registry);
        old_registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut old_registry);

        let mut new_registry = EntityRegistry::new();
        let new_pawn = new_registry.spawn(Transform::default());
        new_registry
            .set_component(new_pawn, health(100.0, 100.0))
            .unwrap();
        restore_carried_health(seats.carried_state(Seat(0)), &mut new_registry, new_pawn);
        seats.bind_pawn(&mut new_registry, Seat(0), new_pawn);

        assert_health_eq(
            new_registry
                .get_component::<HealthComponent>(new_pawn)
                .unwrap()
                .current,
            37.5,
        );
    }

    #[test]
    fn fresh_remote_seat_keeps_descriptor_health_default() {
        let mut seats = SeatTable::from_test_session_id([8; 16]);
        let seat = seats
            .admit_or_reclaim(41, None, false)
            .expect("seat space remains")
            .seat;
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, health(100.0, 100.0)).unwrap();

        restore_carried_health(seats.carried_state(seat), &mut registry, pawn);

        assert_health_eq(
            registry
                .get_component::<HealthComponent>(pawn)
                .unwrap()
                .current,
            100.0,
        );
        assert!(seats.carried_state_for_test(seat).is_none());
    }

    #[test]
    fn harvest_missing_pawn_preserves_prior_health() {
        let mut seats = SeatTable::from_test_session_id([9; 16]);
        let mut registry = EntityRegistry::new();
        let first_pawn = registry.spawn(Transform::default());
        registry
            .set_component(first_pawn, health(100.0, 42.0))
            .unwrap();
        seats.bind_pawn(&mut registry, Seat(0), first_pawn);
        seats.harvest_pawn(&registry, first_pawn);
        registry.despawn(first_pawn).unwrap();

        seats.harvest_bound_pawns(&registry);
        registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut registry);

        assert_health_eq(
            seats
                .carried_state_for_test(Seat(0))
                .and_then(|state| state.health_current)
                .expect("harvest retains prior health"),
            42.0,
        );
    }

    #[test]
    fn single_player_level_change_carries_health_ammo_and_loadout_without_an_endpoint() {
        let mut seats = SeatTable::from_test_session_id([10; 16]);
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        registry.set_component(pawn, health(100.0, 41.0)).unwrap();
        let mut reserve = AmmoReserve::new();
        reserve.credit("shells", 13);
        reserve.set_exact("rockets", 0);
        registry.set_component(pawn, reserve).unwrap();

        let pistol = registry.spawn(Transform::default());
        registry.set_component(pistol, weapon(4)).unwrap();
        registry
            .set_component(pistol, provenance("pistol"))
            .unwrap();
        let launcher = registry.spawn(Transform::default());
        registry.set_component(launcher, weapon(1)).unwrap();
        registry
            .set_component(launcher, provenance("rocket_launcher"))
            .unwrap();
        let inventory = Inventory {
            wieldables: [
                Some(pistol),
                None,
                Some(launcher),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
            active_slot: 2,
            ..Inventory::default()
        };
        registry.set_component(pawn, inventory).unwrap();
        seats.bind_pawn(&mut registry, Seat(0), pawn);

        seats.harvest_pawn(&registry, pawn);
        registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut registry);

        let carried = seats
            .carried_state_for_test(Seat(0))
            .expect("the no-endpoint level unload keeps the local carried record");
        assert_health_eq(carried.health_current.expect("health carries"), 41.0);
        assert_eq!(carried.reserve.available("shells"), 13);
        assert_eq!(carried.reserve.available("rockets"), 0);
        assert_eq!(carried.wieldables[0].as_deref(), Some("pistol"));
        assert_eq!(carried.wieldables[1], None);
        assert_eq!(carried.wieldables[2].as_deref(), Some("rocket_launcher"));
        assert_eq!(carried.magazines[0], Some(4));
        assert_eq!(carried.magazines[2], Some(1));
        assert_eq!(carried.active_slot, 2);
    }

    // Regression: duplicated spawn-path restores could seed a dead pawn.
    #[test]
    fn nonpositive_carried_health_keeps_descriptor_default() {
        let mut seats = SeatTable::from_test_session_id([11; 16]);
        let mut old_registry = EntityRegistry::new();
        let old_pawn = old_registry.spawn(Transform::default());
        old_registry
            .set_component(old_pawn, health(100.0, 0.0))
            .unwrap();
        seats.bind_pawn(&mut old_registry, Seat(0), old_pawn);
        seats.harvest_pawn(&old_registry, old_pawn);

        let mut new_registry = EntityRegistry::new();
        let new_pawn = new_registry.spawn(Transform::default());
        new_registry
            .set_component(new_pawn, health(100.0, 100.0))
            .unwrap();
        restore_carried_health(seats.carried_state(Seat(0)), &mut new_registry, new_pawn);

        assert_health_eq(
            new_registry
                .get_component::<HealthComponent>(new_pawn)
                .unwrap()
                .current,
            100.0,
        );
    }

    #[test]
    fn level_unload_clears_placement_even_without_a_live_pawn() {
        let mut seats = SeatTable::from_test_session_id([12; 16]);
        seats.placement_assignments.insert(Seat(0), 3);
        let mut registry = EntityRegistry::new();

        seats.clear_pawn_bindings_for_level_unload(&mut registry);

        assert_eq!(seats.placement_assignments.get(&Seat(0)), None);
        assert_eq!(seats.carried_state_for_test(Seat(0)), None);
    }

    #[test]
    fn admission_mints_each_seat_once_and_keeps_session_id() {
        let mut seats = SeatTable::from_test_session_id([3; 16]);
        let first = seats.admit_or_reclaim(11, None, false).unwrap().seat;
        let same = seats.admit_or_reclaim(11, None, false).unwrap().seat;
        let second = seats.admit_or_reclaim(12, None, false).unwrap().seat;

        assert_eq!(first, Seat(1));
        assert_eq!(same, first);
        assert_eq!(second, Seat(2));
        assert_eq!(seats.session_id(), SessionId([3; 16]));
    }

    #[test]
    fn same_poll_rejection_does_not_mint_an_admitted_seat() {
        let mut seats = SeatTable::from_test_session_id([4; 16]);

        let seat = seats.admit_or_reclaim(23, None, true);

        assert_eq!(seat, None);
        assert_eq!(seats.seat_for_client(23), None);
        assert_eq!(seats.carried_state_for_test(Seat(1)), None);
    }

    #[test]
    fn roster_projects_only_seat_status_and_connection_lifecycle() {
        let mut seats = SeatTable::from_test_session_id([6; 16]);
        let remote = seats
            .admit_or_reclaim(
                44,
                Some(ConnectClaim {
                    player_id: postretro_net::wire::PlayerClaimId([0x20; 16]),
                    display_name: "Runner".to_owned(),
                }),
                false,
            )
            .expect("seat namespace has room")
            .seat;

        assert!(seats.take_roster_dirty());
        assert!(!seats.take_roster_dirty(), "a poll publishes at most once");
        assert_eq!(
            seats.roster_entries(),
            vec![
                RosterEntry {
                    seat: 0,
                    connected: true,
                },
                RosterEntry {
                    seat: remote.0,
                    connected: true,
                },
            ]
        );
        let roster = seats.roster_message_for(44);
        assert_eq!(roster.session_id, SessionId([6; 16]));
        assert_eq!(roster.your_seat, Some(remote.0));
        assert_eq!(roster.open_seats, crate::netcode::MAX_CLIENTS as u32 - 1);

        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 44),
            Some(remote)
        );
        assert!(seats.take_roster_dirty());
        assert!(
            !seats.roster_entries()[1].connected,
            "a dropped seat remains rostered but is no longer connected"
        );
    }

    #[test]
    fn admitted_disconnect_without_lifecycle_starts_hold_and_later_releases() {
        let mut seats = SeatTable::from_test_session_id([13; 16]);
        let player_claim = claim([0x13; 16], "Runner");
        let released = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        seats.carried.insert(
            released,
            Some(CarriedState {
                health_current: Some(22.0),
                ..Default::default()
            }),
        );
        let _ = seats.take_roster_dirty();

        // An admitted peer can disconnect mid-load without ever producing a
        // participation lifecycle event. Its transport unbind still starts the
        // hold and makes the roster status disconnected.
        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(released)
        );
        seats.advance_hold_clock(HOLD_WINDOW);
        seats.release_expired_holds();

        assert_eq!(seats.carried_state_for_test(released), None);
        assert!(
            !seats.contains_seat(released),
            "a queued owner-slot addition must not recreate a seat released before its app drain"
        );
        assert_eq!(
            seats.roster_entries(),
            vec![RosterEntry {
                seat: 0,
                connected: true,
            }],
            "expiry removes the held seat from the roster atomically with its state"
        );
        assert!(seats.take_roster_dirty(), "expiry dirties the roster once");

        let fresh = seats
            .admit_or_reclaim(42, Some(player_claim), false)
            .expect("seat namespace has room")
            .seat;
        assert_eq!(fresh, Seat(2), "released seat numbers are never reused");
        assert!(seats.carried_state_for_test(fresh).is_none());
    }

    #[test]
    fn exact_claim_reclaims_before_deadline_expiry() {
        let mut seats = SeatTable::from_test_session_id([14; 16]);
        let player_claim = claim([0x14; 16], "Old name");
        let original = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        let _ = seats.take_roster_dirty();

        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(original)
        );
        seats.advance_hold_clock(HOLD_WINDOW);

        let reclaimed = seats
            .admit_or_reclaim(42, Some(claim([0x14; 16], "New name")), false)
            .expect("deadline-frame admission reclaims before expiry runs")
            .seat;
        seats.release_expired_holds();

        assert_eq!(reclaimed, original);
        assert_eq!(seats.seat_for_client(42), Some(original));
        assert!(
            seats
                .roster_entries()
                .iter()
                .any(|entry| entry.seat == original.0),
            "the reconnected seat remains retained after the expiry sweep"
        );
    }

    #[test]
    fn reconnect_after_deadline_mints_fresh_before_the_expiry_sweep() {
        let mut seats = SeatTable::from_test_session_id([18; 16]);
        let player_claim = claim([0x18; 16], "Runner");
        let expired = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(expired)
        );
        seats.advance_hold_clock(HOLD_WINDOW.saturating_add(Duration::from_millis(1)));

        // Admission runs before the post-poll expiry sweep. A strictly overdue
        // hold is not reclaimable even though the sweep has not removed it yet.
        let fresh = seats
            .admit_or_reclaim(42, Some(player_claim), false)
            .expect("seat namespace has room")
            .seat;
        seats.release_expired_holds();

        assert_eq!(fresh, Seat(2));
        assert_eq!(seats.carried_state_for_test(expired), None);
    }

    #[test]
    fn reclaim_requires_an_exact_whole_player_identity() {
        let mut seats = SeatTable::from_test_session_id([15; 16]);
        let original_claim = claim([0x15; 16], "Runner");
        let original = seats
            .admit_or_reclaim(41, Some(original_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(original)
        );

        let mut nearly_matching_id = [0x15; 16];
        nearly_matching_id[7] = 0x16;
        let fresh = seats
            .admit_or_reclaim(42, Some(claim(nearly_matching_id, "Runner")), false)
            .expect("different player id receives a fresh seat")
            .seat;
        assert_eq!(fresh, Seat(2));
        assert_eq!(seats.seat_for_client(42), Some(fresh));

        let reclaimed = seats
            .admit_or_reclaim(43, Some(original_claim), false)
            .expect("the exact opaque identity reclaims its held seat")
            .seat;
        assert_eq!(reclaimed, original);
    }

    #[test]
    fn live_identity_collision_mints_a_fresh_seat_without_displacing_holder() {
        let mut seats = SeatTable::from_test_session_id([16; 16]);
        let player_claim = claim([0x16; 16], "Runner");
        let held_by_live_client = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;

        let logs = LogCapture::start();
        let fresh = seats
            .admit_or_reclaim(42, Some(claim([0x16; 16], "Also Runner")), false)
            .expect("live identity collision receives a fresh seat")
            .seat;

        assert_eq!(held_by_live_client, Seat(1));
        assert_eq!(fresh, Seat(2));
        assert_eq!(seats.seat_for_client(41), Some(held_by_live_client));
        assert_eq!(seats.seat_for_client(42), Some(fresh));
        logs.assert_logged_once(
            log::Level::Warn,
            "asserted a player identity already held by a live connection",
        );
    }

    #[test]
    fn held_seat_keeps_per_seat_values_until_expiry_then_a_fresh_seat_uses_defaults() {
        let mut seats = SeatTable::from_test_session_id([0x19; 16]);
        let player_claim = claim([0x19; 16], "Hold Runner");
        let original = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        let mut slots = StateSlotTable::new();
        slots
            .insert_namespace(
                "currency",
                vec![("xp".to_string(), per_seat_number_slot(0.0))],
            )
            .unwrap();
        slots
            .get_mut("currency.xp")
            .unwrap()
            .set_per_seat_value(original, SlotValue::Number(80.0));

        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(original)
        );
        assert_eq!(
            slots.get("currency.xp").unwrap().per_seat_value(original),
            Some(&SlotValue::Number(80.0)),
            "disconnect starts a hold without clearing the seat-keyed value"
        );

        let reclaimed = seats
            .admit_or_reclaim(42, Some(player_claim.clone()), false)
            .expect("matching claim reclaims the held seat");
        assert_eq!(reclaimed.seat, original);
        assert!(reclaimed.released_seats.is_empty());
        crate::clear_released_seat_slot_values(&mut slots, reclaimed.released_seats);
        assert_eq!(
            slots.get("currency.xp").unwrap().per_seat_value(original),
            Some(&SlotValue::Number(80.0)),
            "reclaim has no restore step because the original map entry remains live"
        );

        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 42),
            Some(original)
        );
        seats.advance_hold_clock(HOLD_WINDOW);
        crate::clear_released_seat_slot_values(&mut slots, seats.release_expired_holds());
        assert_eq!(
            slots.get("currency.xp").unwrap().per_seat_value(original),
            Some(&SlotValue::Number(0.0)),
            "expiry removes the held seat's authoritative entry"
        );

        let fresh = seats
            .admit_or_reclaim(43, Some(player_claim), false)
            .expect("released seat numbers are never reused");
        assert_ne!(fresh.seat, original);
        assert_eq!(
            slots.get("currency.xp").unwrap().per_seat_value(fresh.seat),
            Some(&SlotValue::Number(0.0)),
            "a post-expiry rejoin starts at the declared default"
        );
    }

    #[test]
    fn most_recent_matching_hold_wins_and_releases_stale_duplicate() {
        let mut seats = SeatTable::from_test_session_id([17; 16]);
        let player_claim = claim([0x17; 16], "Runner");
        let stale = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(stale)
        );

        // Build the pre-existing duplicate-hold state this recovery rule must
        // tolerate. Normal admissions cannot create it because the first hold
        // would be reclaimed; the table still needs a deterministic repair.
        let recent = seats
            .admit_or_reclaim(42, Some(claim([0x18; 16], "Other")), false)
            .expect("seat namespace has room")
            .seat;
        seats.connect_claims.insert(recent, player_claim.clone());
        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 42),
            Some(recent)
        );

        let mut slots = StateSlotTable::new();
        slots
            .insert_namespace(
                "currency",
                vec![("xp".to_string(), per_seat_number_slot(0.0))],
            )
            .unwrap();
        slots
            .get_mut("currency.xp")
            .unwrap()
            .set_per_seat_value(stale, SlotValue::Number(11.0));
        slots
            .get_mut("currency.xp")
            .unwrap()
            .set_per_seat_value(recent, SlotValue::Number(29.0));

        let reclaimed = seats
            .admit_or_reclaim(43, Some(player_claim), false)
            .expect("matching hold reclaims");

        assert_eq!(reclaimed.seat, recent);
        assert_eq!(reclaimed.released_seats, vec![stale]);
        crate::clear_released_seat_slot_values(&mut slots, reclaimed.released_seats);
        assert_eq!(
            slots.get("currency.xp").unwrap().per_seat_value(stale),
            Some(&SlotValue::Number(0.0)),
            "the stale duplicate loses its value at reclaim, not at a later expiry sweep"
        );
        assert_eq!(
            slots.get("currency.xp").unwrap().per_seat_value(recent),
            Some(&SlotValue::Number(29.0)),
            "cleanup for the losing seat never touches the reclaimed winner"
        );
        assert_eq!(seats.carried_state_for_test(stale), None);
        assert!(
            seats
                .roster_entries()
                .iter()
                .all(|entry| entry.seat != stale.0),
            "the stale duplicate is released immediately rather than left held"
        );
    }

    #[test]
    fn roster_recipient_gate_excludes_pending_and_closed_slots() {
        assert!(!is_roster_recipient(None));
        assert!(!is_roster_recipient(Some(SlotState::Pending)));
        assert!(is_roster_recipient(Some(SlotState::Admitted)));
        assert!(is_roster_recipient(Some(SlotState::Participating)));
        assert!(!is_roster_recipient(Some(SlotState::Closed {
            cause: postretro_net::slots::CloseCause::Disconnect,
        })));
    }

    #[test]
    fn placement_assignment_persists_by_seat_and_skips_live_and_held_occupants() {
        let mut seats = SeatTable::from_test_session_id([12; 16]);
        let first = seats
            .admit_or_reclaim(41, None, false)
            .expect("first remote seat")
            .seat;
        let second = seats
            .admit_or_reclaim(42, None, false)
            .expect("second remote seat")
            .seat;
        let third = seats
            .admit_or_reclaim(43, None, false)
            .expect("third remote seat")
            .seat;

        assert_eq!(seats.assign_placement(first, 3, []), Some(0));
        assert_eq!(
            seats.carried_state_for_test(first),
            None,
            "placement alone must not make an empty carried record authoritative"
        );
        assert_eq!(
            seats.assign_placement(second, 3, [0]),
            Some(1),
            "a live pawn at index zero keeps a new seat away from it"
        );
        assert_eq!(
            seats.hold_disconnected_client(&mut EntityRegistry::new(), 41),
            Some(first)
        );
        assert_eq!(
            seats.assign_placement(third, 3, [1]),
            Some(2),
            "a held seat reserves its old placement even with no live pawn"
        );
        assert_eq!(
            seats.assign_placement(first, 3, []),
            Some(0),
            "a fresh connection can recover the placement through its durable seat"
        );

        seats.clear_pawn_bindings_for_level_unload(&mut EntityRegistry::new());
        let post_level_admission = seats
            .admit_or_reclaim(44, None, false)
            .expect("a post-level admission mints a fresh seat")
            .seat;
        assert_eq!(
            seats.assign_placement(post_level_admission, 3, [0, 1]),
            Some(2),
            "a post-level admission scans live pawn occupancy before assigning a spawn"
        );
    }

    // Regression: movement away from the authored origin made an occupied spawn look free.
    #[test]
    fn live_placement_occupancy_survives_pawn_movement() {
        let mut seats = SeatTable::from_test_session_id([13; 16]);
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        seats.bind_level_spawn_placement(pawn, 0);
        registry
            .set_component(
                pawn,
                Transform {
                    position: glam::Vec3::new(9.0, -2.0, 4.0),
                    ..Transform::default()
                },
            )
            .unwrap();

        assert_eq!(
            seats.occupied_live_placements(&registry, 2),
            HashSet::from([0])
        );
    }

    #[test]
    fn namespace_exhaustion_marks_roster_dirty_for_seatless_recipient() {
        let mut seats = SeatTable::from_test_session_id([14; 16]);
        let _ = seats.take_roster_dirty();
        seats.next_seat = SEAT_NAMESPACE_SIZE;

        assert_eq!(seats.admit_or_reclaim(99, None, false), None);
        assert!(seats.take_roster_dirty());
        assert_eq!(seats.roster_message_for(99).your_seat, None);
    }

    /// Registry slot index of an id. The reverse index is keyed by whole
    /// `EntityId`, so index reuse is the case a stale entry could be matched by.
    fn slot_index(id: EntityId) -> u32 {
        id.to_raw() & 0xffff
    }

    #[test]
    fn rebinding_a_seat_leaves_the_outgoing_live_pawn_without_an_owner() {
        let mut seats = SeatTable::from_test_session_id([20; 16]);
        let mut registry = EntityRegistry::new();
        let seat = seats
            .admit_or_reclaim(41, None, false)
            .expect("seat namespace has room")
            .seat;
        let old_pawn = registry.spawn(Transform::default());
        let new_pawn = registry.spawn(Transform::default());

        seats.bind_pawn(&mut registry, seat, old_pawn);
        // A demotion without a lifecycle cleanup event leaves the old pawn live
        // while its seat re-promotes onto a freshly materialized one.
        seats.bind_pawn(&mut registry, seat, new_pawn);

        assert_eq!(registry.seat_for_pawn(old_pawn), None);
        assert_eq!(registry.seat_for_pawn(new_pawn), Some(seat));
        assert_eq!(
            [old_pawn, new_pawn]
                .into_iter()
                .filter(|pawn| registry.seat_for_pawn(*pawn) == Some(seat))
                .count(),
            1,
            "the reverse index is many-to-one; exactly one pawn may own a seat"
        );
    }

    #[test]
    fn binding_a_pawn_to_an_unminted_seat_records_no_owner() {
        let mut seats = SeatTable::from_test_session_id([21; 16]);
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());

        seats.bind_pawn(&mut registry, Seat(9), pawn);

        assert_eq!(
            registry.seat_for_pawn(pawn),
            None,
            "the seat-keyed no-op and the reverse index stay in lockstep"
        );
    }

    #[test]
    fn holding_a_disconnected_client_clears_its_still_live_pawns_owner() {
        let mut seats = SeatTable::from_test_session_id([22; 16]);
        let mut registry = EntityRegistry::new();
        let seat = seats
            .admit_or_reclaim(41, Some(claim([0x22; 16], "Runner")), false)
            .expect("seat namespace has room")
            .seat;
        let pawn = registry.spawn(Transform::default());
        seats.bind_pawn(&mut registry, seat, pawn);

        // A drop while demoted reaches the transport edge with no lifecycle
        // cleanup, so the pawn outlives the hold.
        assert_eq!(
            seats.hold_disconnected_client(&mut registry, 41),
            Some(seat)
        );

        assert!(registry.exists(pawn));
        assert_eq!(
            registry.seat_for_pawn(pawn),
            None,
            "a held seat has no live owner to address"
        );
    }

    #[test]
    fn reclaiming_a_held_seat_gives_its_replacement_pawn_the_owner_again() {
        let mut seats = SeatTable::from_test_session_id([23; 16]);
        let mut registry = EntityRegistry::new();
        let player_claim = claim([0x23; 16], "Runner");
        let seat = seats
            .admit_or_reclaim(41, Some(player_claim.clone()), false)
            .expect("seat namespace has room")
            .seat;
        let old_pawn = registry.spawn(Transform::default());
        seats.bind_pawn(&mut registry, seat, old_pawn);
        assert_eq!(
            seats.hold_disconnected_client(&mut registry, 41),
            Some(seat)
        );

        let reclaimed = seats
            .admit_or_reclaim(42, Some(player_claim), false)
            .expect("the exact player identity reclaims its held seat")
            .seat;
        let new_pawn = registry.spawn(Transform::default());
        seats.bind_pawn(&mut registry, reclaimed, new_pawn);

        assert_eq!(reclaimed, seat);
        assert_eq!(registry.seat_for_pawn(new_pawn), Some(seat));
        assert_eq!(registry.seat_for_pawn(old_pawn), None);
    }

    #[test]
    fn level_unload_leaves_no_owner_for_an_entity_reusing_the_recycled_index() {
        let mut seats = SeatTable::from_test_session_id([24; 16]);
        let mut registry = EntityRegistry::new();
        let pawn = registry.spawn(Transform::default());
        seats.bind_pawn(&mut registry, Seat(0), pawn);

        registry.clear_for_level_unload();
        seats.clear_pawn_bindings_for_level_unload(&mut registry);

        let recycled = registry.spawn(Transform::default());
        assert_eq!(
            slot_index(recycled),
            slot_index(pawn),
            "the unloaded pawn's slot index returns to circulation"
        );
        assert_ne!(recycled, pawn);
        assert_eq!(registry.seat_for_pawn(recycled), None);
        assert_eq!(registry.seat_for_pawn(pawn), None);
    }
}
