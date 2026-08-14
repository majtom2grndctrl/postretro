# Per-Player Persistence (E16)

## Goal

Per-owner slot values die with the session. A player's earned currency, XP, or progression is lost on engine exit and cannot travel to another host's game. Give per-owner slots a save/restore lifecycle so values survive to disk and, in co-op, travel with the player to a new session via a join seed carried at admission.

## Prerequisites

- **`E16--per-player-currency`** (shipped) — per-owner slot cardinality, per-seat storage keyed by `Seat`, the `perOwner + persist` load error this spec unlocks, and the `clear_released_seat_slot_values` call sites.
- **`E15--seat-session-identity-roster`** (shipped) — `Seat` identity, `SeatTable`, and the `ConnectClaim` carrying `PlayerClaimId` at admission. This spec keys saved per-owner values by `PlayerClaimId`, the device-local identity `ConnectClaim` already carries.
- **`descriptor-identity-and-naming-sugar`** (shipped) — the durable-key identity ledger (`identity.json`) used by the existing persistence layer. Per-owner saves reuse the same durable keys, extended with the player identity.

## Scope

### In scope

- **Per-owner save — every player saves their own values.** A host or single-player process saves its own per-owner slot values on clean exit, keyed by the local player's `PlayerClaimId`. A connected client saves its own per-owner values periodically (~60 s) and on clean exit, scoped to per-owner slots only — global slots remain host-authoritative and are not saved by the client. The client writes to the same `state.json` file and path the host uses, distinguished by mod id.
- **Per-owner restore on boot.** The local player's saved per-owner values restore at the same lifecycle point global slots restore — after the first successful mod-init commit. The local player's `PlayerClaimId` from player options keys the lookup. Loaded `PersistedState` is retained in the session for join-seed assembly and periodic saves.
- **Join seed.** A connecting client carries its own saved per-owner values in a new app-protocol message sent during the content-parity stage. The host applies valid entries to the client's newly admitted seat, gated by declaration compatibility. The join seed makes player progress portable across hosts.
- **Unlocking `perOwner + persist`.** Remove the `store_bridge.rs` load error that rejects the combination, and update SDK typedefs to allow it.
- **On-disk format extension.** The `state.json` format gains a `per_owner` section keyed by durable slot key, then player identity, then value. Format version bumps from 2 to 3; version-2 files load normally (they contain no per-owner entries).
- **Reference persistence in the dev mod.** The dev mod's per-player XP slot gains `persist: true` and the walkthrough notes session-crossing behavior.

### Out of scope

- **Account identity / cloud sync.** `PlayerClaimId` is device-local. An account-scoped identity needs its own spec; this keying migrates cleanly to it.
- **Conflict resolution across hosts.** Each player saves their own per-owner values to their own device. Two different hosts never write the same player's save file. The only source of divergence is a stale join seed vs. freshly earned values; the periodic client save limits staleness to ~60 s.
- **Per-owner `onStateCrossing`.** Deferred by the currency spec to its own future spec.
- **Exposing `PlayerClaimId` to scripts.** The identity is an engine-side key, never surfaced in the SDK.

## Direction

**Problem.** The persistence layer (`state_persistence.rs`) reads and writes only `record.value` — the single scalar projection per slot. Per-owner slots store values in `per_seat_values`, a map from `Seat` to value, which the save path never touches. And `Seat` is a session-scoped integer with no meaning outside the session that minted it. Both gaps must close: the save/restore path must reach per-seat storage, and saved entries must be keyed by a durable cross-session identity instead of `Seat`.

**Prior commitments.**
- *The guard was placed for this spec.* The currency spec added the `perOwner + persist` load error in `store_bridge.rs` and explicitly deferred the combination to a future persistence spec. This spec unlocks it.
- *A connected client does not write the save file.* `should_save_persisted_state` returns false for a connected client. This spec scopes the rule: global slots remain host-authoritative (the client never saves them), but per-owner values are the player's own data — the client saves those. The rule's intent (prevent clients persisting replicated server-authoritative state) is preserved; its letter is narrowed.
- *`PlayerClaimId` already flows at admission.* A 16-byte device-local identity is generated once per device, persisted in player options, encoded into the `ConnectClaim`, and decoded by the host at `admit_or_reclaim`. This spec reads it — it mints no new identity.
- *The durable-key ledger already keys saved values.* `identity.json` maps authored slot names to opaque durable keys. Per-owner saves extend this: `(durable_key, player_identity)` is the compound key, so a slot rename preserves saved per-owner values the same way it preserves global ones.
- **Divergence, named:** the on-disk format version bumps from 2 to 3. Version-2 documents still load (they have no `per_owner` section); a version-3 document loaded by an older engine is rejected by the existing strict version check (`persisted.version != CURRENT_STATE_VERSION`), which is the correct degradation — an older engine cannot restore values it does not understand.

**Alternatives rejected.**
- *Host saves all players' values; no join seed.* Simpler, but a player's progress is locked to one host's save file. A different host or the player's own single-player session cannot see it.
- *Host saves guest values, client reconciles later.* Requires reconciliation when the player visits a different host and returns — two save files with conflicting values and no arbiter. Eliminated by having each player save their own.
- *A new cross-session identity separate from `PlayerClaimId`.* `PlayerClaimId` is already generated, persisted, and flowing through admission. Minting a second identity for the same purpose is waste.

## Decisions

- **Per-owner values are keyed on disk by `PlayerClaimId`, hex-encoded.** A 16-byte identity becomes a 32-character hex string as the JSON key. Hex is deterministic, human-readable in the save file, and avoids base64 padding ambiguity. The local player's identity comes from `player_options.player_id`.
- **The save document gains a `per_owner` section.** Structure: `{ version: 3, slots: { ... }, per_owner: { durable_key: { hex_player_id: value, ... }, ... } }`. The `slots` section is unchanged. `per_owner` is a nested map: outer key is the slot's durable key (from `identity.json`), inner key is hex `PlayerClaimId`, value is `PersistedValue`.
- **Version-2 files load without error.** An absent `per_owner` section is equivalent to an empty one — no per-owner values to restore. The version check in `overlay_persisted_state` accepts both 2 and 3.
- **Each player saves only their own per-owner values.** The host saves its own per-owner entries (keyed by its `PlayerClaimId` from player options) alongside global slots at clean exit. A connected client saves its own per-owner entries periodically (~60 s) and at clean exit. No player saves another player's data.
- **The Phase 3.5 client-no-save rule is scoped, not reversed.** `should_save_persisted_state` continues to gate the global-slot save path. A new parallel path handles the client's per-owner save: it collects only `perOwner + persist` slots, reads only the local seat's per-owner entry, and writes the `per_owner` section. The `slots` section (global slots) is not touched by the client save.
- **Periodic client save limits progress loss.** A connected client saves its per-owner values every ~60 s and at clean exit. Abnormal termination loses at most ~60 s of per-owner changes, acceptable for a co-op-with-friends trust model.
- **Retained `PersistedState`.** The `PersistedState` loaded at boot is retained in the session (alongside `StateStoreLifecycle`). The periodic save merges fresh per-owner values into the retained document's `per_owner` section before writing. The join seed reads from the retained document's `per_owner` section at connection time. The retained document is main-thread-only and is never accessed from a background worker.
- **`Seat(0)` uses the local player's `PlayerClaimId`.** `Seat(0)` has no `ConnectClaim` — it is never admitted through the admission path. Its identity comes from `player_options.player_id` directly.
- **The `is_persisted_mod_slot` filter excludes per-owner slots.** The existing global-slot collect/overlay loops filter by `is_persisted_mod_slot`, which checks `persist`, `readonly`, and `ownership` but not `per_owner`. Per-owner slots must not enter the global `slots` section — they save into `per_owner` instead. Add a `!record.schema.per_owner` check to `is_persisted_mod_slot`.
- **The join seed is a single app-protocol message.** A new `ClientControlMessage::JoinSeed { slots: BTreeMap<String, JoinSeedValue> }` sent by the client during the parity stage, before the connection transitions to Participating. The keys are durable slot keys from `identity.json`; values are `JoinSeedValue`-encoded. The host validates each entry against declared schemas and applies valid entries to the client's newly minted seat. Invalid entries warn and are skipped.
- **The join seed carries only `perOwner + persist` slots.** A global `persist` slot is host-authoritative. A per-owner non-persist slot is runtime-only. The intersection — per-owner and persistent — is the join seed's payload.
- **The client builds the join seed from the retained `PersistedState`.** At connection, the client extracts its own per-owner entries (keyed by its own `PlayerClaimId`) from the retained document and sends them as the seed. A client with no save file sends an empty seed — per-owner slots start at defaults.
- **The join seed is sent alongside parity, not after it.** The seed is not functionally dependent on parity — the host validates entries against its own declarations. The client sends the seed early on the same reliable Control channel as parity messages, so the host has it buffered by the time `SlotEvent::Participating` fires. The host applies the buffered seed in the `Participating` handler between seat lookup and pawn spawn — after the seat is known but before the player materializes. If the seed has not arrived when `Participating` fires (defensive fallback), the host spawns with defaults and applies the seed when it arrives on a subsequent poll — `set_per_seat_value` works regardless of pawn state.
- **A reclaim within the hold window does not apply the join seed.** The seat's per-owner values are already live from the previous connection. The join seed may be stale (saved at the client's last periodic save, not at disconnect). The live values take precedence.
- **`JoinSeedValue` is a wire-level enum in `postretro-net`.** Same variants as `PersistedValue` minus `Unsupported` (Boolean, Number, String, Array). Converted at the boundary in the binary crate via `From<JoinSeedValue> for PersistedValue` (wire to internal) and `TryFrom<PersistedValue> for JoinSeedValue` (internal to wire — `Unsupported` entries are skipped with a warning). Avoids a crate dependency cycle — `postretro-net` does not depend on `postretro` or `postretro-scripting-core`.
- **Wire backward compatibility for the join seed.** `ClientControlMessage` is a bitcode-encoded enum. bitcode has no self-describing unknown-variant skip — an older host decoding a `JoinSeed` variant it does not recognize fails and rejects the client (`transport.rs` closes the connection). The engine is pre-stable; mixed-version co-op is not a supported configuration. No wire version negotiation or opaque extension payload is needed. An older client that never sends a seed starts at defaults — that direction degrades cleanly.
- **`main.rs` is past the size guidance (~10,700 lines).** The persistence changes are localized to `state_persistence.rs` and thin call-site wiring in `main.rs`. No split-first task — the extensions are small and touch existing call sites.

## Acceptance criteria

- [ ] AC 1: A `perOwner + persist` slot declaration is accepted at mod-init; the combination that was previously rejected now loads.
- [ ] AC 2: In single-player, a per-owner slot's value survives engine exit and restart — the restored value matches what was saved.
- [ ] AC 3: In single-player, a per-owner slot's value survives a level transition within the same session (already shipped; this confirms persistence does not regress it).
- [ ] AC 4: A global persistent slot saves and restores identically to today — the per-owner extension does not alter global-slot behavior. (Review gate: byte-identical global-slot `slots` section when no per-owner slots are declared.)
- [ ] AC 5: A version-2 `state.json` loads without error under the new format; per-owner slots start at declared defaults. A version-3 file with per-owner entries loads and restores those entries.
- [ ] AC 6: The save document's `per_owner` section is keyed by durable slot key and hex-encoded `PlayerClaimId`; a slot rename (durable key preserved) restores the renamed slot's per-owner values.
- [ ] AC 7: A connected client saves its own per-owner values periodically (~60 s) and at clean exit. The save document's `slots` section is empty or absent — the client does not save global slots.
- [ ] AC 8: A seat with no valid `PlayerClaimId` (anonymous connection) does not contribute per-owner entries to the save document.
- [ ] AC 9: In co-op, a connecting client sends a join seed carrying its saved per-owner values. The host applies valid entries to the client's freshly minted seat; invalid entries (type mismatch, out-of-range value, undeclared slot, non-persist slot) warn and are skipped.
- [ ] AC 10: A client with no save file sends an empty join seed; all per-owner slots start at their declared defaults.
- [ ] AC 11: A reclaim within the hold window does not apply the join seed — the seat's live per-owner values are preserved from the previous connection.
- [ ] AC 12: After a join-seed apply, the client's HUD shows the restored per-owner values (owner-private replication delivers the applied values).
- [ ] AC 13: A client that does not send a seed (older engine or no save file) starts at defaults on the host. An older host rejects a newer client that sends `JoinSeed` — expected pre-stable incompatibility; no wire version negotiation needed.
- [ ] AC 14: The periodic save timer fires within [55, 65] seconds of the previous save, and each save writes a valid `per_owner` section to disk.
- [ ] AC 15: The dev mod's per-player XP slot declares `persist: true`, and XP survives across single-player sessions.

## Tasks

### Task 1: Unlock `perOwner + persist` and extend the on-disk format

Remove the `perOwner + persist` rejection in `crates/scripting-core/src/store_bridge.rs` (the `if per_owner && persist { return Err(...) }` guard). Update SDK typedefs in both TypeScript (`expected.d.ts`) and Luau (`expected.d.luau`) to allow `persist?: boolean` on `perOwner: true` slot variants — the TypeScript `persist?: never` constraint on those union arms becomes `persist?: boolean` (Luau equivalent: `persist: nil?` becomes `persist: boolean?`). Update the committed typedef assertion test in `crates/postretro/src/scripting/typedef/tests/committed.rs` to expect `persist?: boolean` / `persist: boolean?` on the `perOwner: true` arms. The remaining combination rejections (`perOwner + accumulate`, `perOwner + network: "shared"`) stay in place.

Extend `PersistedState` with a `per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>` field, `serde(default)` and `serde(skip_serializing_if = "BTreeMap::is_empty")`. Outer key: durable slot key. Inner key: hex-encoded `PlayerClaimId`. Bump `CURRENT_STATE_VERSION` from 2 to 3 and widen the acceptance guard in the same commit. Update the version check in `overlay_persisted_state` (line 151: `persisted.version != CURRENT_STATE_VERSION`): accept both 2 and 3 — reject anything outside that range. A version-2 document has no `per_owner` field; `serde(default)` yields an empty map, so global-slot restore proceeds unchanged and per-owner slots start at defaults.

Add `is_persisted_per_owner_slot` — a filter matching `persist && per_owner && !readonly && ownership == Mod`. Update `is_persisted_mod_slot` to exclude per-owner slots: add `&& !record.schema.per_owner`. This prevents per-owner slots from entering the global `slots` section. Both filters live in `state_persistence.rs`.

Make `PersistedValue` `pub(crate)` (currently module-private) so the per-owner collect/overlay functions and the join-seed conversion can reference it from outside the module.

Add a `PlayerClaimId` hex-encode utility — a standalone function in `state_persistence.rs` that takes `&PlayerClaimId` (imported from `postretro_net::wire` — `postretro` already depends on `postretro-net`) and returns a `String`, plus its decode inverse for join-seed validation. The call sites convert `player_options.player_id: Option<[u8; 16]>` to `PlayerClaimId` at the boundary.

Retain the loaded `PersistedState` in the session. Currently `overlay_persisted_state` borrows the loaded document (`&PersistedState`) and drops it after the overlay completes — it is not retained in the session. Add a new `Option<PersistedState>` field to `Session` in `session/mod.rs`, alongside the existing `state_store_lifecycle` field. Change the boot path in `splash_lifecycle.rs` to store the loaded `PersistedState` into this field after the overlay completes. The periodic save and join-seed assembly read from it; the save path updates it.

Test: a `PersistedState` with `per_owner` entries round-trips through serialize/deserialize. A version-2 document deserializes with an empty `per_owner`. Hex encode/decode round-trips. The SDK typedef changes compile. `is_persisted_mod_slot` returns false for a `perOwner + persist` slot. `is_persisted_per_owner_slot` returns true for such a slot and false for global-only or non-persist slots.

### Task 2: Save and restore per-owner state

**Collect.** Add `collect_per_owner_state` in `state_persistence.rs`. For each `perOwner + persist` slot (filtered by `is_persisted_per_owner_slot`), read the local seat's per-owner entry via `record.per_seat_value(local_seat)`, convert to `PersistedValue`, and insert under `(durable_key, hex_player_id)`. The function takes `&SlotTable`, `Option<&StoreIdentityLedger>`, `&BTreeSet<String>` (committed membership), `local_seat: Seat`, and `local_player_id: [u8; 16]`. Returns a `BTreeMap<String, BTreeMap<String, PersistedValue>>` (the `per_owner` shape). The call site no-ops when `player_options.player_id` is `None` — no per-owner values are collected or saved.

The collect function reads only the local seat's per-owner entries — each player saves their own values. On the host, `local_seat` is `Seat(0)` and `local_player_id` is from player options. On a connected client, `local_seat` is the client's own seat and `local_player_id` is from player options.

**Restore.** Extend `overlay_persisted_state` to restore per-owner entries. After the existing global-slot overlay loop, add a per-owner pass:
1. For each entry in `persisted.per_owner`, resolve the durable key to an authored slot name via the identity ledger.
2. Confirm the slot is `perOwner + persist` and writable (not readonly, mod-owned).
3. For each `(hex_player_id, persisted_value)` under that durable key, check if `hex_player_id` matches the local player's identity.
4. If it matches: validate the value via `restored_value` (type, range, enum membership), then write it into the local seat's per-seat storage via `set_per_seat_value(local_seat, value)`.
5. If it does not match: skip — other players' entries are preserved in the save file but not loaded into storage.

`overlay_persisted_state`'s signature gains `local_player_id: Option<[u8; 16]>` and `local_seat: Seat`. The call site in `splash_lifecycle.rs` passes player options' `player_id` and `Seat(0)`.

**Clean-exit save (host and single-player).** Update the save path in `main.rs`'s `exiting()`. After the existing `collect_persisted_state` call (which collects global slots), call `collect_per_owner_state` for the local player. Merge the result into `collected.state.per_owner`. The host saves only its own per-owner entries — each guest saves their own values via their own periodic and clean-exit saves. Write the combined document via `save_persisted_state` as today.

**Periodic save (connected client).** Add a periodic save path for connected clients. A timer (~60 s) triggers a per-owner-only save: call `collect_per_owner_state` for the local player, build a save document with `version: CURRENT_STATE_VERSION`, an empty `slots` section, and the collected `per_owner` entries merged with the retained `PersistedState`'s `per_owner` section (preserving other players' entries). Write via `save_persisted_state`. The client never collects or writes global slots — the `slots` section is always empty in the client's save, even if the retained document carried global slots from a prior single-player session. The timer resets after each save. Clean exit also triggers the same save. The periodic save fires once per frame, after all fixed ticks have drained and the `Rc<RefCell<SlotTable>>` is not borrowed — specifically, after the second system-command drain and before the impact-policy discard, where both the `SlotTable` and registry `RefCell`s are unborrowed. The timer runs only while the client is in the Participating state; a demotion (level change) pauses the timer, re-promotion resumes it, and reconnection zeros the timer's elapsed accumulator but does not change its running/paused state (a reconnection during Loading zeros the accumulator; the timer remains paused until the client re-enters Participating).

The periodic save needs `state_path` (from `state_persistence`), mod identity, the identity ledger, committed store slots, the slot table, the local seat, and the local player id. Thread these through the connected-client code path. `should_save_persisted_state` is not changed — it continues to gate the global-slot save. The periodic per-owner save is a separate code path that runs regardless of `is_connected_client`.

**Client clean-exit save.** At `exiting()`, a connected client runs the same per-owner-only save as the periodic path (one final save before shutdown). This runs even though `should_save_persisted_state` returns false for a connected client — the per-owner save is outside that gate.

Test: a round-trip save then fresh-table restore recovers per-owner values for the local player. Entries for other player ids are preserved in the save document but not loaded into the local seat's per-seat storage. A `perOwner` slot without `persist` is not restored. A version-2 document with no `per_owner` section restores global slots normally and leaves per-owner slots at defaults. Range clamping, type mismatch, and enum validation apply to per-owner entries the same as global ones. A connected client's save document contains only the `per_owner` section (empty `slots`). Periodic save updates the `per_owner` section without touching `slots`.

### Task 3: Join seed — client send and host apply

**Wire type.** Define `JoinSeedValue` in `crates/net/src/wire/control.rs` (or a sibling module in the wire crate) with variants: Boolean(`bool`), Number(`f64`), String(`String`), Array(`Vec<f64>`) — matching `PersistedValue` minus `Unsupported`. Derive bitcode `Encode`/`Decode`. Append `JoinSeed { slots: BTreeMap<String, JoinSeedValue> }` as the last variant of `ClientControlMessage`, preserving the positional discriminant of existing variants (`Admission`, `Parity`, `SwitchDeclaration`). Bump `WIRE_VERSION` — the new variant changes the bitcode layout of `ClientControlMessage`. The wire crate (`postretro-net`) does not depend on `postretro` or `postretro-scripting-core`, so `JoinSeedValue` is a parallel type converted at the boundary. Add `From<JoinSeedValue> for PersistedValue` (wire to internal, for host apply) and `TryFrom<PersistedValue> for JoinSeedValue` (internal to wire, for client send — `Unsupported` entries are skipped with a warning) in the binary crate. `PersistedValue` is `pub(crate)` in `state_persistence.rs` (made visible by Task 1); the conversion impls live in the same crate.

**Client side.** After content-parity agreement, the client builds the join seed from the retained `PersistedState`. Extract the `per_owner` section, filter to entries matching the client's own `PlayerClaimId`, convert each `PersistedValue` to `JoinSeedValue` via `TryFrom` (skipping `Unsupported` entries), and package as `ClientControlMessage::JoinSeed`. Send via `client.send_message(Channel::Control, wire::encode(&message))` — the same reliable channel the parity declaration uses. The send is alongside parity, before the client transitions to participating. A client with no save file (retained `PersistedState` is `None` or has an empty `per_owner`) sends a `JoinSeed` with an empty map.

**Host side.** When the host receives `JoinSeed`:
1. The net crate passes the seed through `ServerPoll.join_seeds`. The binary crate buffers it per-connection in a `HashMap<ClientId, BTreeMap<String, JoinSeedValue>>`. The message arrives during the parity stage, before the connection transitions to Participating. The buffer holds at most one seed per connection; a subsequent `JoinSeed` from the same connection (e.g., client re-enters parity on level change) replaces the buffer entry. A disconnect clears the entry.
2. When `SlotEvent::Participating` fires for this connection, apply the buffered seed in the `Participating` handler between seat lookup and pawn spawn — after the seat is known but before the player materializes. This is the same handler seam where the carried loadout is threaded (`main.rs` `SlotEvent::Participating` handler, ~5499–5615). For each seed entry:
   a. Resolve the durable key to an authored slot name via the identity ledger.
   b. Confirm the slot is `perOwner + persist`, writable, and mod-owned.
   c. Convert `JoinSeedValue` to internal representation, validate via `restored_value`.
   d. If valid, write into the seat's per-seat storage via `set_per_seat_value`.
   e. If invalid, warn and skip.
3. The seed is not functionally dependent on parity — the host validates each entry against its own declarations, not against parity outcome. The client sends the seed early (alongside parity, not after it) so the host has it buffered by the time `Participating` fires. Both are `ClientControlMessage` variants on the same reliable Control channel; in the normal case the seed arrives well before `Participating`. If the seed has not arrived when `Participating` fires (defensive fallback — transport anomaly or future transport change), spawn with defaults but leave one late-first-seed opportunity. When that first seed arrives on a subsequent poll, apply it to the existing seat — `set_per_seat_value` borrows only the `SlotTable` via `script_ctx.slot_table.borrow_mut()`, never the entity registry, and works regardless of pawn state. The next owner-private replication cycle delivers the values to the client.
4. If the seat was **reclaimed** within the hold window, discard the seed with a log note — live values from the carried snapshot harvested at disconnect take precedence.
5. Applying a buffered seed at `Participating` consumes that connection's seed opportunity. Defaulting because no seed was buffered does not consume it: exactly one subsequently arriving first seed may apply (or be rejected because the seat was reclaimed), then the opportunity is consumed. Later `JoinSeed` duplicates are dropped. A demotion (level change) clears the consumed marker and buffer entry, so a client re-entering parity gets a fresh seed opportunity for its next `Participating` transition.
6. After apply, the next owner-private replication cycle delivers the values to the client's HUD.

The net crate's `process_control_messages` (`transport.rs`) handles the new `JoinSeed` variant in its `ClientControlMessage` match: pass the seed through `ServerPoll` as a new field `join_seeds: Vec<(ClientId, BTreeMap<String, JoinSeedValue>)>`, following the `switch_declarations` passthrough precedent. The net crate does not interpret or buffer the seed — it is registry-blind. The binary crate buffers incoming seeds per-connection in a `HashMap<ClientId, BTreeMap<String, JoinSeedValue>>` maintained alongside `finish_host_poll`. A subsequent seed for the same connection replaces the buffer entry; a disconnect clears it. The binary crate's `Participating` handler in `main.rs` takes from this buffer by `client_id` and applies entries using the slot table, identity ledger, and committed membership.

A client that never sends `JoinSeed` (older engine, no save file) starts at defaults — no timeout or fallback needed. An older host rejects the client at decode — an expected pre-stable incompatibility.

Test: a client with saved per-owner values sends a join seed; the host applies them to the client's seat; the client's HUD shows the restored values. A client with no save file sends an empty seed; values start at defaults. A reclaimed seat (hold window) does not apply the seed. Invalid seed entries (wrong type, out of range, non-persist slot) are skipped with warnings. A second `JoinSeed` arriving after a seed has applied or been rejected for reclaim is discarded. An older host rejects the client (pre-stable incompatibility); an older client starts at defaults. When the seed arrives before `Participating`, it is applied before the pawn spawns; after defaulting, the first late-arriving seed is applied on receipt and later duplicates are dropped.

### Task 4: Dev mod reference persistence

Update the dev mod's per-player XP slot declaration in `content/dev/scripts/combat-lifecycle.ts` to add `persist: true`. The shared `teamKills` slot stays non-persistent (session-scoped by design). Update the dev HUD walkthrough in `content/dev/README.md` (or equivalent) to note that XP persists across sessions and travels with the player via join seed. Verify single-player XP survives an engine restart.

## Sequencing

**Phase 1 (sequential):** Task 1 — format extension, filter updates, retained state, and `PersistedValue` visibility that every later task consumes.
**Phase 2 (sequential):** Task 2 — save, restore, and periodic save. Both Tasks 2 and 3 modify `state_persistence.rs`; sequencing avoids merge conflicts.
**Phase 3 (sequential):** Task 3 — join seed. Consumes the save/restore infrastructure from Task 2. The client's send path reads from the retained `PersistedState`; the host's apply path writes per-seat storage.
**Phase 4 (sequential):** Task 4 — reference persistence. Consumes all of it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A per-owner value saved to disk is keyed by a durable cross-session identity, not by session-scoped `Seat` | Task 1 (hex `PlayerClaimId` keying), Task 2 (collect resolves local player id) | Any path that writes per-owner entries using `Seat` as the key instead of `PlayerClaimId` | AC 6 |
| Global-slot persistence is behaviorally unchanged | Task 1 (`is_persisted_mod_slot` excludes per-owner; format extension is additive), Task 2 (per-owner passes are separate functions) | A per-owner code path that touches `record.value` instead of `per_seat_values` corrupts global slots; a client's periodic save that writes the `slots` section overrides host-authoritative values | AC 4, AC 7 |
| A connected client saves per-owner values but never global slots | Task 2 (periodic save collects only per-owner; client clean-exit save does the same) | A code path that calls `collect_persisted_state` (global) on a connected client | AC 7 |
| The join seed does not overwrite a held seat's live values | Task 3 (reclaim-within-hold-window skip) | A reclaim path that applies the seed before checking hold status loses in-session progress | AC 11 |
| Version-2 save files load without error | Task 1 (`serde(default)` on `per_owner`, version check accepts 2 and 3) | A version check that rejects 2 breaks existing saves | AC 5 |
| Per-owner slots do not enter the global `slots` section | Task 1 (`is_persisted_mod_slot` adds `!per_owner` check) | Removing the check or adding a per-owner slot to the global collect loop | AC 4 |
| Both periodic save and clean-exit save run synchronously on the main thread | Task 2 (save path is synchronous `fs::write` + `fs::rename`) | Making the periodic save async (e.g., spawning to a thread pool) introduces a race between periodic and exit saves | AC 7, AC 14 |
| Each connection gets one seed application opportunity per participation generation | Task 3 (apply consumes; defaulting leaves one late-first-seed opportunity) | Consuming on default drops the first delayed seed; failing to consume after apply or reclaim rejection lets duplicates override live values | AC 9, AC 11 |

## Ordering pins

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Client periodic save fires during gameplay | Timer elapses; `collect_per_owner_state` reads the local seat's per-owner entries; merges into retained `PersistedState.per_owner`; writes to disk | Save document reflects current per-owner values; `slots` section unchanged from boot |
| Client periodic save fires, then abnormal termination | Save completed; crash occurs before next periodic save | At most ~60 s of per-owner changes lost; values from last periodic save survive on disk |
| Client clean exit | `exiting()` triggers per-owner-only save (same as periodic path); writes to disk | Per-owner values saved; no global slots saved |
| Host clean-exit save with global and per-owner slots | `collect_persisted_state` collects global slots into `slots`; `collect_per_owner_state` collects the host's per-owner entries into `per_owner`; combined document written | Both sections present; per-owner entries keyed by host's `PlayerClaimId` |
| Client connects, join seed arrives before `Participating` | Client sends `JoinSeed` during parity; host buffers it; `SlotEvent::Participating` fires; host applies buffered seed between seat lookup and pawn spawn | Client's per-owner values are live before the pawn materializes |
| Client connects, join seed arrives after `Participating` (defensive fallback) | Both on same reliable channel; transport anomaly delays seed; `Participating` fires before seed arrives; host spawns with defaults; seed arrives on subsequent poll; host applies via `set_per_seat_value` | Per-owner values applied late; next owner-private replication delivers them to client. Brief window where defaults are visible |
| Client reconnects within hold window, join seed arrives | `admit_or_reclaim` reclaims the held seat; host discards the buffered seed | Live per-owner values from the previous connection preserved; stale seed ignored |
| Client reconnects after hold expiry, join seed arrives | Hold expired; seat released and per-owner values cleared; `admit_or_reclaim` mints fresh seat; host applies the seed | Client's saved values from the seed applied to the fresh seat |
| Host exits cleanly with a guest still connected | Host saves its own per-owner entries via `collect_per_owner_state`; guest's per-owner entries are not saved by the host (the guest saves its own) | Host's save document contains only the host's per-owner entries |
| Single-player restore with per-owner entries for multiple players | `overlay_persisted_state` restores only entries matching the local player's `PlayerClaimId` into `Seat(0)`; other players' entries left in the retained document | Only the local player's values are live; others' entries preserved for future join seeds |
| Abnormal termination (host or single-player) | No save runs | All unsaved per-owner changes lost, matching the global-slot contract |
| Abnormal termination (connected client) | Last periodic save is the recovery point | At most ~60 s of per-owner changes lost |
| Two players with the same `PlayerClaimId` (device cloned) | Both seats write the same hex key in the `per_owner` section; last writer wins | Degenerate case, no worse than the single-player case; `PlayerClaimId` is documented as device-local |
| Host mutates a client's per-owner slot; client saves before replication arrives | Host writes `xp = 100` at tick T; client's local `xp` is still 80 (pre-replication); client periodic save fires; client saves `xp = 80` | Client's save file contains the stale value. On reconnect, join seed carries 80, potentially rolling back the host's write. Bounded by ~60 s + replication latency; acceptable for the co-op trust model |
| Periodic save fires on a frame with multiple fixed ticks | Tick 1 mutates X from 10 to 20; tick 2 mutates X to 30; after all ticks drain, periodic save reads X = 30 | Save captures the final post-tick value, never an intermediate. The save runs after all ticks drain (post-game-logic, pre-render seam) |
| Periodic save timer fires during Loading state (client demoted) | Client demoted on level change; timer paused; per-owner values survive in slot table but save does not fire | Timer resumes on re-promotion; next save captures current values |
| Client sends two `JoinSeed` messages on the same connection (re-enters parity on level change) | Client sends seed A during first parity; host level change demotes client; client re-enters parity, sends seed B; host admits client | Host replaces buffered seed A with seed B; applies seed B to the fresh seat |
| `collect_per_owner_state` runs with `player_options.player_id` as `None` | Local player has no device identity | Call site no-ops; no per-owner values collected or saved |
| Same-poll disconnect + reconnect (same `PlayerClaimId`, different transport `client_id`) | Disconnect fires `hold_disconnected_client` (harvests carry, clears pawn); handshake fires `admit_or_reclaim` (reclaims within hold); `Participating` fires (restores carried state) | Reclaim succeeds; carried snapshot restored. Join seed discarded (reclaim path). Single-poll, no observable pawn gap |
| `JoinSeed` arrives, `Participating` applies it, then second `JoinSeed` arrives | First seed buffered; `Participating` consumes buffer; second seed arrives post-apply | Second seed discarded. Host does not re-apply |
| `exiting()` fires on a frame where periodic save already wrote | Periodic save completes (synchronous, main-thread); `exiting()` collects and writes again | Exit save overwrites periodic save. On-disk state is the later (exit) snapshot. No corruption — both are synchronous and main-thread-only |
| Host reverts client's per-owner slot; client saved stale-high value before revert snapshot arrived | Client saves high value; revert snapshot arrives next frame; client's on-disk state is stale-high | Next join seed carries stale-high value. Host validates seed entries (AC 9: out-of-range values are skipped). Bounded by ~60 s + replication latency |
| Periodic save fires on a zero-tick frame (frame_dt < tick_dt) | Timer advances by frame_dt; elapsed >= 60 s; save fires at post-tick seam (which still runs, 0 tick iterations) | Save reads current slot values (no mutation this frame). Valid |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| per-owner persistence (declaration) | `SlotSchema.persist` + `SlotSchema.per_owner` (both true) | slot declaration key `persist` alongside `perOwner` | `{ perOwner: true, persist: true }` | same |
| player identity (save key) | `PlayerClaimId([u8; 16])` -> hex `String` | hex-encoded 32-char string as JSON key in `per_owner` section | -- (not author-facing) | -- |
| join seed message | `ClientControlMessage::JoinSeed { slots }` | bitcode-encoded `BTreeMap<String, JoinSeedValue>` | -- (not author-facing) | -- |
| join seed value type | `JoinSeedValue` (wire crate) <-> `PersistedValue` (binary crate) | bitcode `JoinSeedValue` enum: Boolean, Number, String, Array | -- | -- |
| on-disk format | `PersistedState { version: 3, slots, per_owner }` | JSON with `per_owner: { durable_key: { hex_id: value } }` | -- (not author-facing) | -- |
| wire version | `WIRE_VERSION` bumped (new `ClientControlMessage` variant changes bitcode layout) | -- | -- | -- |
