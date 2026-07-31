# Seat, Session Identity, and Roster

## Goal

Give the engine a durable per-player key. Today every per-player address dies at a boundary: entity ids die at level unload, connection ids die at reconnect, and no identity is assertable by the player at all. This spec mints three separate identities — session, seat, player — and uses the seat to carry health, ammo, and loadout across a level change and back to a player who drops and rejoins.

## Scope

### In scope

- Host-minted session id, one per hosted run.
- Host-minted seat, one per participant, surviving level transitions.
- Client-asserted device-local player id, carried in the connect token.
- Engine-owned roster keyed by seat, published to clients.
- Seat-hold window and reclaim-by-player-id on rejoin.
- Carried per-player state: current health, ammo reserve across all ammo types, per-slot magazines, inventory composition, active slot.
- Three shipped defect fixes: the placement-assignment survival claim, terminal closed slots, and the client's unreconciled ammo shadow copy.
- Behavior-preserving splits of the four production files this spec extends.

### Out of scope

- **Per-seat scalar state slots and the `perOwner` mod-store authoring surface.** `drafts/E16--per-player-currency` owns them. This spec mints the key that surface will use; it does not build the surface. The two are separable because the carried set here is structured (a map, two arrays, an index) and has no scalar-slot representation — see Direction.
- **Lobby UI, join predicate, session-phase vocabulary.** Spec 3 of the band, deferred there by `plans/done/E15--session-lifecycle/index.md:72-75`.
- **Authentication.** `context/lib/index.md:102` names anti-cheat a non-goal of this project's multiplayer; `research/coop-session-lobby.md:80-84` states the friends-group trust posture and says plainly that the player id "is not an authentication mechanism." The connect token is unsecured and its claim is forgeable. Two forward-compat constraints are honored instead — invariants I3 and I4.
- **Host migration and session-state serialization.** `plans/done/E15--session-lifecycle/index.md:78-80` opened the ledger without serializing it. This spec adds two entries to that ledger and keeps the roster rebuildable (I5); it writes no serializer.
- **A split of `crates/postretro/src/main.rs`.** Owner decision. This spec touches it in three narrow places — the participation seam, the per-client message drain, and remote command preparation — and a 9,967-line split is its own project.
- **Client-side health as a component.** A connected client's pawn has never carried `HealthComponent`; health reaches it only as a replicated slot. Carry stays host-side.
- **Authored carry policy.** Carrying is unconditional here. Pistol-start, per-level resets, and any authored opt-out are policy, not mechanism — see Direction. The seam is named and kept to one call per spawn path; no descriptor field and no knob ship in this spec.

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Session id | `SessionId([u8; 16])` — new, `crates/net/src/wire.rs` | `[u8; 16]` | n/a | n/a | n/a |
| Seat | `Seat(u16)` — new, `crates/foundation` | bare `u16`, no type | n/a | n/a | n/a |
| Player id | `PlayerClaimId([u8; 16])` — new, `crates/net/src/wire.rs` | `[u8; 16]` | n/a | n/a | n/a |
| Connect claim | `ConnectClaim` — new, `crates/net/src/wire.rs` | bitcode blob inside connect-token `user_data` | n/a | n/a | n/a |
| Roster entry | `RosterEntry` — new, `crates/net/src/wire.rs` | struct in `Vec<RosterEntry>` | n/a | n/a | n/a |
| Roster message | `ServerControlMessage::SessionRoster` — new, appended | positional tag, appended last | n/a | n/a | n/a |
| Seat record | `SeatSessionState` — new, `crates/postretro/src/netcode/seat.rs` | **never crosses the wire** | n/a | n/a | n/a |
| Carried loadout | `CarriedLoadout` — new, binary-local | never | n/a | n/a | n/a |
| Replication scope (engine) | `postretro_entities::slot_table::ReplicationScope` (`slot_table.rs:52`, has `None`) | — | — | — | — |
| Replication scope (wire) | `postretro_net::state_slots::ReplicationScope` (`state_slots.rs:76`, no `None`) | — | — | — | — |

Two distinct types share the name `ReplicationScope`. They are not interchangeable and `entities` does not import net's; the mapping lives in the binary. Any task naming the type must qualify which one.

**`Seat` is the one identity that does not live in the net crate, and it must not acquire a second definition there.** `net` depends only on renet, renet_netcode, bitcode, and log — it is postretro-free by contract (`development_guide.md:28`) and cannot take a `foundation` dependency. `entities` may depend only on `foundation` (enforced by `layering_invariants_hold`, `crates/xtask/src/crate_graph.rs:496`). So no crate exists that both `net` and `entities` can name a shared type from. `foundation` is the only home from which the binary, `entities`, and any later floor-crate consumer can all name one `Seat`; the wire carries a bare `u16` and the net crate defines no seat type at all. Minting a `Seat` in `net` would force a duplicate in `entities` the first time per-seat storage reaches the floor slot table — the same split this table already documents for `ReplicationScope`, and the one I2 forbids.

`SessionId` and `PlayerClaimId` stay in `net`: they cross the wire and nothing below the binary names them.

Display names are UTF-8 `String` on the wire and are never used as a key.

## Wire format

Two new surfaces.

### Connect claim — connect-token `user_data`

Fixed 256 bytes (`NETCODE_USER_DATA_BYTES`), carried in `ClientAuthentication::Unsecure`, populated at `crates/net/src/transport.rs:764` where the client currently passes `None`. The host reads it with `transport.user_data(client_id)` on the `ClientConnected` edge in `collect_server_events` (`transport.rs:270-281`).

| Offset | Width | Content |
|---|---|---|
| 0..4 | 4 | Magic `b"PRSC"` |
| 4 | 1 | Format version, `u8`, currently `1` |
| 5..7 | 2 | Payload length, `u16` little-endian |
| 7..7+len | len | bitcode-encoded `ConnectClaim` |
| 7+len..256 | rest | Zero fill, ignored on read |

`ConnectClaim` derives native `bitcode::Encode`/`Decode` like every other wire type (`wire.rs:4`), holds `player_id: [u8; 16]` and `display_name: String`, and gains fields only by appending — bitcode tags positionally (`wire.rs:1048-1049`).

The magic prefix is load-bearing and not decoration. When a client passes `user_data: None`, renetcode fills the 256 bytes with **random** data rather than zeros, so an absent claim is indistinguishable from a present one without a marker. Magic mismatch, version mismatch, a length exceeding the remaining bytes, or a bitcode decode failure all mean *no claim*, and the host mints an anonymous seat that can never be reclaimed. This is the degradation path for a dev launch with no configured identity.

Endianness applies to exactly one field, the length prefix. No other multi-byte integer appears in the outer layout — the player id is opaque bytes and the payload is bitcode's own encoding — so there is one endianness decision, not a family of them.

This layout mirrors no existing section, because no existing section is a fixed-width transport field. The framing choice follows `ParityDeclaration` (`wire.rs:1110`) in using a bitcode struct for the payload, and adds the magic/version/length envelope only because the 256-byte field is fixed-width and pre-filled with noise.

### Session roster — `ServerControlMessage::SessionRoster`

A new variant **appended after `SwitchAccepted`** (`wire.rs:1260`), carrying `SessionRosterMessage { session_id: SessionId, your_seat: Option<u16>, entries: Vec<RosterEntry> }`. `RosterEntry` holds `seat: u16`, `player_id: Option<PlayerClaimId>`, `display_name: String`, `connected: bool`. Seats travel as bare `u16` because the net crate cannot name the `Seat` type — see the Boundary inventory.

Conventions followed from the crate: variable-length lists are a `Vec` of a named struct, never parallel arrays (`ShotVerdictsMessage`, `wire.rs:1087`); an empty `Vec` is valid and means an empty roster; `Option` carries an explicit documented meaning — `your_seat: None` means the recipient holds no seat yet, `player_id: None` means an anonymous seat. Fixed-size identity is a bare byte array, not a `Vec` (`ParityDeclaration.mod_digest`, `wire.rs:1111`).

### Tuning payload extension

`WieldableTuningPayload` (`tuning_payload.rs:21-29`) gains a loaded-magazine count and a reserve count, both appended after the existing fields because bitcode encodes struct fields positionally. The payload is an opaque `Vec<u8>` inside `ServerControlMessage::Tuning`, so this changes no envelope — only the blob's inner layout, which both ends agree on via the protocol version admission already gates.

Reserve travels **per inventory slot**, carrying the balance for that slot's ammo type, rather than as a map of ammo type to balance. Two slots sharing a type carry the same number. This keeps a string-keyed map off the wire entirely and matches the shape the client already consumes, since it composes per slot from this same payload.

`SeatSessionState` never crosses the wire. The net crate stays registry-blind (I8); it carries seat *ids*, never seat *contents*.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| I1 — A seat survives every exit from participating except a close | Task 2 (seat table), Task 7 (release path) | `networking.md:98` clears all slot state on *any* exit from participating, and a level change is now such an exit; a release rule keyed to "leaves participating" releases every seat at every level change | AC-SEAT-2, AC-CARRY-1 |
| I2 — The seat is the sole key joining the roster to carried per-player state | Task 2, Task 6 (roster) | Any second per-player key added beside it re-creates the duplicate-store problem this spec exists to end | AC-ROSTER-1, AC-REJOIN-1 |
| I3 — The player id is opaque | Task 1 (claim decode), Task 7 (reclaim) | The engine must never parse, slice, derive from, or order by its bytes; equality at the reclaim chokepoint is the only permitted operation | AC-ID-3 |
| I4 — Seat reclaim is decided at exactly one chokepoint | Task 7 | Any second site that grants a seat from a claim makes a future auth change a multi-site edit | AC-ID-3, AC-ID-1 |
| I5 — The roster is rebuildable from the client-asserted player ids alone | Task 6 | Session id and seats are host-minted and cannot survive a host change (`roadmap.md:202`); a roster deriving anything from a seat that is not also derivable from its player id breaks this | AC-ROSTER-2 |
| I6 — The within-level player address is unchanged | Task 2 | E17's `PlayerId` (`trigger_system.rs:22`) has three named consumers — trigger occupancy, the Use seam, and per-player `use_pressed`; the seat is a parallel key and replaces none of them | AC-E17-1 |
| I7 — Carried state is an enumerable set, not a scattered accretion | Task 2, Task 5 | `roadmap.md:185` requires the session-state set be enumerable because host migration is that set plus a live-world layer; a value that carries without appearing in the ledger violates it | AC-LEDGER-1 |
| I8 — The net crate stays registry-blind | Task 1, Task 6 | `development_guide.md:28`, enforced by `layering_invariants_hold` (`crates/xtask/src/crate_graph.rs:496`); seat ids may cross the wire, seat contents may not | AC-BUILD-1 |

## Ordering matrix

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Harvest vs. slot-map teardown | `clear_net_level_parity` is the first statement of `unload_level` (`startup/lifecycle.rs:238`) and replaces `SlotPawns` wholesale (`netcode/mod.rs:949`), destroying the client→pawn map before any entity despawns | Harvest runs ahead of that statement. A harvest placed later finds no pawn for any client and silently carries nothing |
| Harvest vs. entity despawn | Entities stay live until `clear_for_level_unload` (`startup/lifecycle.rs:280-285`); `data_registry.clear()` runs earlier, at `:274` | Harvest reads components before `:274` so descriptor lookups still resolve |
| Demote and close on the same tick | Both events drain from the same lifecycle queue | Close wins. The seat enters its hold window; the demote is absorbed |
| Level change while a seat is held | Hold is session-scoped; the level boundary does not touch it | Held seat keeps its last harvested state and seeds nothing, because it owns no pawn |
| Level change with no pawn for a slot | A level may resolve no `player_spawn`, leaving a fly-camera and no pawn (`entity_model.md:198`) | Harvest finds no pawn and leaves the seat's prior state intact rather than overwriting it with empty |
| Hold expires during a level install | Expiry is evaluated at one per-tick chokepoint | Release never lands mid-install; it applies at the next chokepoint after the install completes |
| Reclaim after hold expiry | Seat already released and removed from the roster | Fresh seat at descriptor defaults. No error; the player simply starts over |
| Two live connections assert the same player id | Second claim arrives while the first holder is still connected | Second gets a fresh seat, no reclaim, and the host logs a warning. The first holder is never displaced |
| Rejoin before the host processed the close | renet mints a new client id; the prior slot is still participating or admitted | Treated as the two-live-connections row above until the prior slot times out |
| Claim absent or undecodable | Magic mismatch, version mismatch, over-long length, or bitcode failure — including the random bytes renetcode writes for `user_data: None` | Anonymous seat, no reclaim, warning logged once per connection |
| Reclaim vs. a never-connected tombstone | `transition` plants a permanent `Closed` tombstone for ids that never connected, to refuse stale packets (`slots.rs:79-82`) | Reclaim operates on seats, and a tombstone has no seat, so the two populations never interact. Making a closed slot reclaimable must not resurrect a tombstone |
| Session with zero remaining participants | Every seat held or released; roster may be empty | Session id persists for the process. An empty roster is a valid roster |
| Roster publish vs. entry to participating | A slot must be participating to receive control frames with a live epoch (`transport.rs:633-642`) | Roster publishes on entry to participating and on every roster change thereafter; a not-yet-participating slot is not sent one |

## Direction

**Problem.** The engine has no key that names a player for longer than one level or one connection, so every feature needing per-player state either dies at a boundary or is parked.

**Prior commitments.** The session-state ledger (`networking.md:149-153`) holds one entry, the connection, and says later specs add the seat and the roster to it — this spec is that addition. The admission/parity split turns on mutability (`networking.md:75-87`): the player id is *carried, never compared*, so it gates nothing and cannot convert a recoverable difference into a disconnect. `roadmap.md:202` requires the roster be rebuildable from player ids alone, and I5 honors it. E17 said not to invent a heavyweight identity system (`plans/done/E17--trigger-command-surface/index.md:361-368`); I6 keeps its address intact and adds a parallel key.

Two divergences, both deliberate.

*`networking.md:102` reads stronger than it is.* It says a client id is stable across a level change, "which is what later specs key player identity to." Client ids are wall-clock nanos minted per connection (`netcode/mod.rs:642`), so that stability holds within one connection and not across a rejoin. This spec keys the *seat* to the connection and the *reclaim* to the player id. The doc line needs narrowing at promotion.

*Terminal closed slots become reclaimable.* `SlotState::Closed` is terminal by two independent mechanisms — an early return in `transition` (`slots.rs:69-72`) and `entry().or_insert()` in `on_connect` (`slots.rs:59`) — and `SlotTable` has no `remove`, `clear`, or `retain`, so the map grows for the endpoint's lifetime. Reclaim requires relaxing both in a coordinated way, and the never-connected tombstone population must stay terminal. This is a shipped contract change and the riskiest edit in the spec.

*Placement in the layer stack.* Three placements, decided separately because they answer to different constraints. The seat **table** lives in the binary's `netcode/` module: it holds engine types, and `entities` is a compile chokepoint with six dependents including the whole render and UI stack that `development_guide.md:41` keeps domain logic out of. The nearest existing thing to a seat — `placement_assignments`, a durable per-client map surviving a close (`netcode/lifecycle.rs:35-42`) — is already there. The seat **id type** lives in `foundation`, because it is the only crate both `entities` and the binary can name, and per-seat storage reaches the floor slot table as soon as the currency spec lands. Session id and player id live in `net` alongside the other wire identities, mirroring `NetworkId` — a wire type whose allocator lives in the binary. Recorded as a fact for the reviewer, not as a cleared question.

*Carry is mechanism; whether a level carries is policy.* This spec makes the seat able to carry and makes carrying unconditional, matching the continuous-progression shape the campaign wants today. Pistol-start is a live design axis for a Doom-lineage shooter, and the repo's mechanism-versus-policy thesis is explicit — `E16--impact-policy-substrate` builds a substrate holding no opinion about what a hit means. The opt-out seam is named rather than built: seeding reads the seat's carried record through one call per spawn path, so a policy that suppresses carry for a level suppresses that read. No authored knob ships here; adding one is a descriptor field and a check at that call, not a redesign.

**Alternatives rejected.**

*Key everything by the client-asserted player id and mint no seat.* One identity instead of three, trivial rejoin, a trivially rebuildable roster. Rejected because the player id is client-chosen and forgeable: making it the live key means a forged claim steals *live* session state rather than merely claiming a released one, and it makes admission stop being a host decision. The seat gives the host a value it controls. It also makes the id load-bearing everywhere, which is precisely what a later auth epic would have to unwind.

*Carry state through the existing `persist` save path.* Rejected on two shipped rules: a connected client does not write the save file (`should_save_persisted_state`), and client-to-host state writes are a stated Phase 3.5 non-goal. Persist is also machine-scoped where the carry is session-scoped — the wrong lifetime.

*Carry through per-seat state slots instead of a structured record.* The strongest rival, and the one that would also serve the currency spec. Its upside is real: the state store has engine-global lifetime and is never cleared on level unload (`scripting.md:110`), which would delete the harvest hook, the ordering constraint against `unload_level`'s first statement, and most of the harvest rows in the Ordering matrix. Owner-private replication to the owning client already ships, which is much of Task 8.

Rejected on shape, stated precisely because the loose version of this claim is wrong. `SlotType::Array` does exist (`slot_table.rs:20-27`), so magazines fit, and health and active slot are `Number`. But `SlotValue::Array` is `Vec<f32>` (`slot_table.rs:15`) — floats only. Inventory composition is a list of canonical *name strings* and has no representation at all, and the ammo reserve is a map keyed by ammo-type string, which no slot type expresses. Composition is the load-bearing value: without it the seat cannot rebuild a loadout, which is the carry's whole point. So a structured record is required regardless of what else moves into slots, and taking this path would also mean absorbing the `perOwner` authoring axis that `drafts/E16--per-player-currency` owns.

The seat still stands as the key a scalar per-seat axis will use when that spec builds one — same key, two shapes, one store each with no overlap in what they hold.

**Forecloses.** Fixing the claim in the connect token means it cannot change mid-connection — intended, and what makes it admission-stage-legal. The 256-byte budget forecloses a large profile payload; a 16-byte id plus a display name leaves roughly 230 bytes spare. Monotonic non-reused seats foreclose a dense seat index; nothing wants one. Nothing here forecloses an authored carry policy: the seed path is one call per spawn path, and suppressing it is a check at that call.

**One-way doors.** The connect-claim layout is versioned and the version byte is checked, so a format change costs a protocol bump that admission already gates. Relaxing closed-slot terminality is the genuine door: it weakens a guarantee other code may lean on, and re-tightening it later would remove rejoin. `Seat`'s crate home is a door in practice rather than in principle — the type is trivial to move, but once consumers build against a net-crate seat, the duplicate-type shape is what gets extended; placing it in `foundation` now costs nothing. The carry inversion is fully reversible — deleting the harvest and seed hooks restores descriptor seeding.

## Acceptance criteria

- [ ] **AC-CARRY-1** — A player crossing a level boundary keeps their current health, their ammo reserve for every ammo type they hold, each weapon's loaded magazine, their weapon loadout, and the weapon they were holding.
- [ ] **AC-CARRY-2** — A player at full defaults, a player mid-magazine, and a player at zero reserve all carry correctly; a level that spawns no pawn leaves prior carried state unchanged rather than clearing it.
- [ ] **AC-SEAT-1** — Every participant holds a distinct seat, and a released seat's number is never reissued for the life of the session.
- [ ] **AC-SEAT-2** — A host level change does not release any seat, and every participant's seat is the same before and after.
- [ ] **AC-REJOIN-1** — A player who disconnects and rejoins within the hold window resumes with the health, ammo, and loadout they left with, at their previous spawn placement.
- [ ] **AC-REJOIN-2** — A player who rejoins after the hold window expires starts at descriptor defaults, and the session continues without error.
- [ ] **AC-ID-1** — A client launched with no configured identity joins successfully, receives a seat, and is reported as anonymous; on rejoin it receives a new seat rather than reclaiming the old one.
- [ ] **AC-ID-2** — A second client asserting an identity already held by a live participant joins with its own new seat, the existing participant is undisturbed, and the collision is logged.
- [ ] **AC-ID-3** — Reclaim succeeds only on an exact whole-identity match; a claim differing in any byte gets a fresh seat.
- [ ] **AC-ROSTER-1** — The roster lists every seat with its connection state, survives a level change unchanged, and drops a seat when its hold expires.
- [ ] **AC-ROSTER-2** — The roster can be reconstructed from the participants' asserted identities alone, with no dependence on any host-minted value.
- [ ] **AC-LEDGER-1** — The set of values that survive a level transition is enumerated in one place, and adding a value to the carry without adding it to that enumeration fails a test.
- [ ] **AC-E17-1** — Trigger occupancy, the Use interaction, and per-player use state behave exactly as before for both local and remote players across a level change.
- [ ] **AC-CLIENT-1** — A connected client's displayed ammo and reserve match the host's authoritative values after a level change, including for weapons that are not currently equipped.
- [ ] **AC-BUILD-1** — The workspace layering test passes unchanged and the transport crate still compiles with no dependency on the entity registry; each split file's tests pass with identical counts before and after, with no call-site path changes.
- [ ] **AC-HYGIENE-1** — Repeated connect and disconnect cycles do not grow per-connection bookkeeping without bound.

## Tasks

### Task 1: Connect claim on the wire

Add the client-asserted identity to the connect token and read it host-side. Define `PlayerClaimId([u8; 16])`, `SessionId([u8; 16])`, and `ConnectClaim { player_id, display_name }` in `crates/net/src/wire.rs`, deriving native `bitcode::Encode`/`Decode` per that file's convention — no serde, and any future field is appended because bitcode tags positionally. Define `Seat(u16)` in `crates/foundation`, **not** in the net crate: `net` is postretro-free by contract (`development_guide.md:28`) and `entities` may depend only on `foundation` (enforced by `layering_invariants_hold`, `crates/xtask/src/crate_graph.rs:496`), so `foundation` is the only crate from which the binary, `entities`, and later floor-crate consumers can all name one seat type. Seats cross the wire as a bare `u16`; the net crate defines no seat type. Implement the 256-byte envelope: magic `b"PRSC"` at bytes 0..4, version `1` at byte 4, `u16` little-endian payload length at bytes 5..7, bitcode payload following, zero fill to 256. Provide encode and decode helpers where decode returns "no claim" for magic mismatch, version mismatch, a length exceeding the remaining bytes, or a bitcode failure — this matters because renetcode fills an absent `user_data` with **random** bytes, not zeros, so absence is only detectable by the marker. Thread an `Option<[u8; 256]>` parameter through `NetClient::new` (`transport.rs:750-756`, currently hardcoding `user_data: None` at `:764`) to all seven call sites: `netcode/mod.rs:788`, `:3816`, `:3870`, `netcode/trigger_state_channel_harness_test.rs:298`, `net/src/harness.rs:280`, `transport.rs:1038`, plus the doc contract at `netcode/mod.rs:743`. Host-side, read `transport.user_data(client_id)` inside the `ClientConnected` arm of `collect_server_events` (`transport.rs:270-281`) and stash the decoded claim on `NetServer` alongside the existing per-client side tables; the claim must be read on that edge and stashed, because renet_netcode drops the netcode entry when the connection tears down and it is not retrievable later. Confirm `NetcodeServerTransport::user_data`'s exact signature against the compiled crate before relying on it — it was verified from published sources, not from a local checkout. The net crate holds the claim as opaque bytes and never interprets them (I3, I8). Expose a read accessor for the binary. No seat, no roster, no carry in this task.

### Task 2: Seat table, session id, and the thin slice

Build the seat, mint it, and carry exactly one value end to end so the seams are falsified before anything fans out. Create `crates/postretro/src/netcode/seat.rs` owning a `SeatTable`: a monotonic `Seat` allocator that never reuses a number, a map from seat to `SeatSessionState`, a map from seat to the currently bound client id, and the session id minted once when the host endpoint is built. `SeatSessionState` in this task holds only `health_current: Option<f32>`; Task 5 extends it. The table lives in the binary because it holds engine types and because `entities` is a compile chokepoint that domain logic stays out of (`development_guide.md:39-41`). The `Seat` type itself comes from `foundation` (Task 1) and is not redefined here. Mint a seat when a slot first becomes participating, binding it to that client id, and record the claim from Task 1 against it. **A seat must survive every exit from participating except a close** (I1) — `networking.md:98` clears all slot state on any such exit and a level change is now one of them, so a release rule keyed to leaving participation would release every seat at every level change, which is the exact bug the seat exists to prevent. Harvest current health into the seat during level unload, and this placement is not negotiable: `clear_net_level_parity` is the *first* statement of `unload_level` (`startup/lifecycle.rs:238`) and replaces `SlotPawns` wholesale (`netcode/mod.rs:949`), destroying the client→pawn map, so the harvest must run ahead of it; entities themselves stay live until `:280-285`, and descriptor lookups stop resolving after `data_registry.clear()` at `:274`. Seed the harvested health back onto the pawn after spawn on the two paths that attach `HealthComponent` — `spawn_from_player_starts` (`data_archetype.rs:1000`) and `spawn_net_slot_pawn` (`net_descriptor.rs:90`) — as a post-spawn override rather than by threading into `attach_descriptor_components`. Do not add health to the connected-client path: that pawn has never carried `HealthComponent` (`remote_materialize.rs:143-199` attaches movement, inventory, mesh, and viewer role only), and its health arrives as a replicated slot. Keep E17's `PlayerId` untouched (I6). Sequenced after the splits so the edits land in the post-split files.

### Task 3: Split `netcode/mod.rs`

Behavior-preserving split, no functional change. The file is 5,985 lines with production ending at 3,203. Extract along the seams already visible: role and config parsing plus weapon-attachment sync (`:131-313`), the `NetEndpoint` enum and its impl including the three reset functions at `:895`, `:922`, and `:977` (`:315-1135`), id and shot bookkeeping (`:1137-1495`), the client receive/predict/interpolate surface (`:1498-2044`), host tick and accept/register (`:2046-2475`), tuning and switch declarations (`:2477-2643`), and host message ingest with hit validation (`:2645-3065`). The endpoint region is the one this spec extends most, so it must come out cleanly. Move the corresponding `#[cfg(test)] mod tests` blocks with the code they cover. Public and `pub(crate)` paths must keep resolving — re-export from the parent module rather than rewriting call sites across the binary. Run the full crate test suite before and after and confirm identical pass counts.

### Task 4: Split `startup/lifecycle.rs`

Behavior-preserving split, no functional change. The file is 4,407 lines with production ending around 1,934. Extract the path and identity helpers (`:43-94`), the net-parity and unload cluster (`:96-330`, containing `clear_surface_lifetime_level_state`, `unload_level`, and `clear_net_level_parity`), boot-state frame driving (roughly `:340-735`), level payload delivery and install (`:640-1175`), world gravity and nav plus reaction rebinding (`:1183-1300`), spawner resolution (`:1303-1424`), and the CPU install segment including `install_world_cpu` and `install_descriptor_player_health_range` (`:1426-1919`). The unload cluster and the install segment are both extended by later tasks, so both must come out cleanly. Keep the ordering inside `unload_level` byte-for-byte identical — a later task depends on inserting ahead of its first statement, and any reordering here silently changes what a harvest can see. Move tests with their code, keep paths resolving via re-export, and confirm identical pass counts before and after.

### Task 5: Full carried set

Extend the carry from health alone to the whole per-player set: current health, ammo reserve across every ammo type held, each inventory slot's loaded magazine, the inventory composition, and the active slot. Store composition as canonical names, never entity ids — `clear_for_level_unload` despawns everything and bumps generations (`registry.rs:1036-1055`, `:938-943`), so no `EntityId` survives. Canonical names are readable from a live weapon through `DescriptorProvenance.canonical_name` (attached at `data_archetype.rs:705-713`), and the restore path already exists: `compose_wieldable_inventory_from_slots` takes `&[Option<String>; WIELDABLE_SLOT_CAPACITY]` (`data_archetype.rs:112-129`). Seed through the single composition chokepoint `compose_wieldable_inventory_slots` (`data_archetype.rs:131-194`) — all three spawn paths reach it, and it is the only writer of `Inventory` and the only caller of `seed_weapon_reserve`. Two shipped gaps make this more than a hook. `active_slot` is currently *derived* — first populated slot wins, `data_archetype.rs:189-192` — so a carried active slot needs either a parameter or a post-composition override. And `AmmoReserve` exposes only `credit` (saturating add) and `take` (`ammo_reserve.rs:25-39`) with no iterator and no setter, so restoring an exact balance requires either a new set-exact API or replacing the whole component; the component is fully serializable with no skipped fields, so whole-component replacement is available. Magazines restore by writing `WeaponComponent.magazine` (`weapon.rs:275`) per slot after composition. Carry no cooldowns, no in-flight reload state, no movement runtime, no death latch, and no kill credit — `pending_kill_credit` and `contributor_ledger` are already `#[serde(skip)]` and documented transient (`health.rs:328`, `:340`). The carried set must be enumerated in exactly one place such that adding a value without adding it to that enumeration fails a test (I7).

### Task 6: Roster and the session-state ledger

Build the engine-owned roster and publish it. The roster is keyed by seat and holds, per entry, the seat, the asserted player id if any, a display name, and whether that seat is currently connected. Its vocabulary is closed: it carries lifecycle facts the engine mints or observes, and nothing else. **The roster must be reconstructible from the asserted player ids alone** (I5) — session id and seats are host-minted and cannot survive a host change (`roadmap.md:202`), so no roster field may depend on a host-minted value that is not also derivable from its player id. Add the `SessionRoster` variant to `ServerControlMessage`, **appended after `SwitchAccepted`** (`wire.rs:1260`) because bitcode tags enum variants positionally and inserting anywhere else renumbers shipped variants. Publish on entry to participating and on every roster change; do not publish to a slot that is not participating, because a control frame sent to a slot with no participation epoch carries `None` and the client drops it with a warning (`transport.rs:633-642`, `:918-932`). The message carries the recipient's own seat so a client knows which entry is its own. Seat *contents* never cross the wire — the net crate stays registry-blind (I8, `development_guide.md:28`, enforced by `layering_invariants_hold` at `crates/xtask/src/crate_graph.rs:496`). Add the seat and the roster to the session-state ledger enumeration that `networking.md:149-153` opened with one entry. Also fix the placement defect: `SlotPawns.placement_assignments` documents that it "survives a close so a reconnecting client lands on its prior spawn" (`netcode/lifecycle.rs:35-42`), which is false because `reset_level_scoped_host_state` replaces the whole struct (`netcode/mod.rs:949`) — move placement assignment onto the seat, where the claim becomes true, and correct or delete the comment.

### Task 7: Seat hold, reclaim, and closed-slot reclaimability

Make a dropped player able to come back. On a close, do not destroy the seat: harvest its state, mark it disconnected in the roster, and start a hold window. On a new connection whose claim matches a held seat's player id exactly, rebind that seat to the new client id and restore its carried state; on no match, or on an expired hold, mint a fresh seat at defaults. **This decision happens at exactly one chokepoint** (I4) — a later auth epic replaces the claim's provenance there and touches nothing else — and matching is whole-identity equality only, never a parse or a partial comparison (I3). Evaluate hold expiry at a single per-tick site so a release never lands mid-install. This requires relaxing closed-slot terminality in `crates/net/src/slots.rs`, which is guarded two independent ways: an early return in `transition` for any slot already `Closed` (`:69-72`) and `entry().or_insert()` in `on_connect` (`:59`) that refuses to resurrect a closed id. Both must be relaxed together, and `SlotTable` currently has no `remove`, `clear`, or `retain` at all, so the map grows for the endpoint's lifetime — bound it. Critically, `Closed` is **two populations**: genuine closes of slots that once participated, and permanent tombstones planted for ids that were never connected, to refuse stale packets (`:79-82`). Tombstones must stay terminal; only a genuine close may become reclaimable, and a reclaim path that resurrects a tombstone reopens the stale-packet hole. Note that `close_slot` clears `parity_declarations`, `holding_diagnostics`, and `participation_epochs` (`transport.rs:503-508`), so a rebound connection re-establishes parity from scratch like any new connection — the seat carries gameplay state, not handshake state. Two live connections asserting one identity is not an error: the second gets its own fresh seat, the first is undisturbed, and the host logs it.

### Task 8: Reconcile the client's ammo shadow copy

A connected client composes its own inventory and therefore its own `AmmoReserve` and per-slot `magazine` values from descriptor defaults (`net_descriptor.rs:201-221` into `data_archetype.rs:183-187`), while the tuning payload carries neither — `WieldableTuningPayload` holds canonical name, range, cooldown, fire mode, resolution, and lower/raise timings only (`tuning_payload.rs:21-29`). Nothing reconciles them and nothing reads them today, so the divergence is currently invisible. It stops being invisible once the host pawn is seat-seeded: the host's magazines and reserve come from the seat while the client's come from the descriptor, and the client's HUD reads replicated slots that project only the *active* weapon's ammo type (`state_slots.rs:583-618`), leaving every other held ammo type with no wire representation at all. Close the gap by extending the per-wieldable tuning payload, **not** by adding state slots. Add a loaded-magazine count and a reserve count to `WieldableTuningPayload` (`tuning_payload.rs:21-29`), one pair per inventory slot, where the reserve figure is the balance for *that slot's* ammo type. Do not attempt this through the slot table: the reserve is a map keyed by ammo-type string, and no `SlotType` expresses one — `SlotValue::Array` is `Vec<f32>` (`slot_table.rs:15`), so a string-keyed map has no representation and an implementer reaching for the slot path will dead-end. Per-slot framing also avoids putting a map on the wire at all: two inventory slots sharing an ammo type carry the same number, which the client reads per slot and never has to reconcile. The payload is already per-client, reliable, and rebuilt whenever it changes (`host_send_tuning_if_changed`), so no new message and no new send cadence are needed. Both fields are appended to the struct because bitcode encodes positionally, and the protocol handshake already gates a version mismatch before any payload decodes. Inventory *composition* and active slot need no new wire — the client already composes from the tuning payload's canonical names (`net_descriptor.rs:207-213`), and the host builds that payload from its own live inventory, so seeding the host pawn from the seat before tuning is sent makes the client's composition follow. That warrant covers composition only; magazines and reserve genuinely have no path today, which is why this task exists.

### Task 9: Split `wire.rs` control region and the composition block

Two small behavior-preserving extractions, sequenced ahead of the tasks that extend them. In `crates/net/src/wire.rs` (2,959 lines, production 1,335), extract the handshake/parity/control region at `:1100-1295` into a `wire/control.rs` submodule — this is exactly where Tasks 1 and 6 add types. `digest_hex` (`:1235`) is used only by `DivergenceReason`'s `Display` and travels with it; `ServerControlFrame` and `ParticipationFrame` are `pub(crate)` and consumed by `transport.rs` (imported at `:19-20`), so the new module must re-export at crate-internal visibility. In `crates/postretro/src/scripting/builtins/data_archetype.rs` (3,035 lines, production 1,013), extract the inventory and reserve composition block at `:54-194` — `seed_weapon_reserve`, `compose_wieldable_inventory`, `compose_wieldable_inventory_from_slots`, `compose_wieldable_inventory_slots` — which is where Task 5 lands. Watch visibility: several of these are `pub(super)` and need widening or a re-export shim in `builtins/mod.rs`. Both extractions must preserve public and crate-internal paths and produce identical test pass counts.

### Task 10: Carry and rejoin harness coverage

Build the test coverage for the whole flow, which does not exist today. There is no two-process harness: `crates/net/src/harness.rs` is a packet conditioner (delay, jitter, loss on a virtual clock), not a loopback. The real in-process harness is `LoopbackHarness` (`crates/postretro/src/netcode/predict_reconcile_harness_test_fixtures.rs:382`, driven by `step` at `:843` and `step_until_armed` at `:832`), which runs real endpoints over loopback sockets in one process and drives the production seams end to end — but it has no demote/repromote leg and no level-change leg, so both are new. Extend it to cover: a client connecting with an asserted identity and receiving a seat; a level change preserving that seat and its carried values; a disconnect starting a hold; a rejoin inside the window reclaiming the seat and its state; and a rejoin after expiry getting a fresh seat at defaults. Add the ordering cases that unit tests can cover directly rather than through the harness: harvest with no pawn present, an absent or corrupt claim, two live connections asserting one identity, and a reclaim attempt against a never-connected tombstone. Follow the project's test conventions — `<subject>_<verb>_<expected_outcome>` naming, injected fixed delta time rather than wall-clock reads, and exhaustive matches with no wildcard arm so a new state variant becomes a compile error. Any log line an acceptance criterion names becomes contract; keep asserted strings to the criteria that need them.

## Sequencing

**Phase 1 (concurrent):** Task 3, Task 4, Task 9 — behavior-preserving splits of the four files later tasks extend. No functional change, no shared files.

**Phase 2 (sequential):** Task 1 — the claim reaches the host and nothing else changes. Narrowest possible first crossing of the transport seam.

**Phase 3 (sequential):** Task 2 — thin slice; falsifies the boundary assumptions by carrying one value across every seam before anything fans out.

**Phase 4 (concurrent):** Task 5, Task 6, Task 7 — the carried set, the roster, and reclaim. All three consume Task 2's seat table and are independent of each other.

**Phase 5 (sequential):** Task 8 — consumes the seat-seeded host pawn from Task 5.

**Phase 6 (sequential):** Task 10 — consumes every prior phase.

Splits lead rather than following the thin slice because they are behavior-preserving and every subsequent task edits the files they touch; running them after Task 2 would move that task's edits and manufacture conflicts. Task 2 remains the first task that crosses seams and the first that can falsify the design.

## Rough sketch

`Seat(u16)` in `crates/foundation`. New wire types in `crates/net/src/wire.rs` (post-split, `wire/control.rs`): `SessionId`, `PlayerClaimId`, `ConnectClaim`, `RosterEntry`, `SessionRosterMessage`, plus `ServerControlMessage::SessionRoster` appended last.

New module `crates/postretro/src/netcode/seat.rs`: `SeatTable { session_id: SessionId, next_seat: u16, seats: HashMap<Seat, SeatSessionState>, bound: HashMap<Seat, u64>, claims: HashMap<Seat, Option<PlayerClaimId>>, holds: HashMap<Seat, HoldDeadline> }`. `SeatSessionState { health_current: Option<f32>, reserve: AmmoReserve, wieldables: [Option<String>; WIELDABLE_SLOT_CAPACITY], magazines: [Option<u32>; WIELDABLE_SLOT_CAPACITY], active_slot: usize, placement: Option<usize> }`.

Harvest enters `unload_level` ahead of `clear_net_level_parity`; seed applies after `compose_wieldable_inventory_slots` and after the two health-bearing spawn calls. Route every seed through one call per spawn path so a later carry policy has a single site to suppress. `AmmoReserve` gains a set-exact API or is replaced wholesale — it derives `Serialize`/`Deserialize` with no skipped fields, so either works.

## Open questions

- **Hold-window duration.** Not pinned here. It wants a playtest, and the mechanism is indifferent to the number. Suggest a default in the low minutes and make it a constant, not an authored knob — an authored knob is lobby-surface work that spec 3 owns.
- **Whether an anonymous seat should be avoidable.** A dev launch with no configured identity gets a seat it can never reclaim. Generating and persisting a device-local id on first run would remove the case entirely, but writing a new machine-scoped file is `player_options`-shaped work this spec does not otherwise touch.
- **`NetcodeServerTransport::user_data`'s exact signature** was verified against published crate sources, not a local checkout — no cargo registry exists in this environment. Task 1 confirms it at the compiler before relying on it.
- **`drafts/E16--per-player-currency` carries a stale decision.** It states that a disconnect releases the seat and drops its values, and that a rejoin is a new seat at defaults — reversed by Task 7, which holds the seat and restores it. That draft also argues against a standalone seat spec and places the seat in `entities`. All three need reconciling there when it unparks; none blocks this spec.
