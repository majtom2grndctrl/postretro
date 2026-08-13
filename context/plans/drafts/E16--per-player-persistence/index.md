# Per-Player Persistence (E16)

## Goal

Per-owner slot values die with the session. A player's earned currency, XP, or progression is lost on engine exit and cannot travel to another host's game. Give per-owner slots a save/restore lifecycle so a player's values survive to disk and, in co-op, travel with the player to a new session via a join seed carried at admission.

## Prerequisites

- **`E16--per-player-currency`** (shipped) — the per-owner slot cardinality, per-seat storage keyed by `Seat`, the `perOwner + persist` load error this spec unlocks, and the `clear_released_seat_slot_values` call site this spec hooks for capture.
- **`E15--seat-session-identity-roster`** (shipped) — the `Seat` identity, `SeatTable`, and the `ConnectClaim` carrying `PlayerClaimId` at admission. This spec keys saved per-owner values by `PlayerClaimId`, the device-local identity `ConnectClaim` already carries.
- **`descriptor-identity-and-naming-sugar`** (shipped) — the durable-key identity ledger (`identity.json`) used by the existing persistence layer. Per-owner saves reuse the same durable keys, extended with the player identity.

## Scope

### In scope

- **Per-owner save on clean exit.** A host or single-player process captures each live seat's per-owner slot values, keyed by the seat's asserted `PlayerClaimId`, into the existing per-mod `state.json` save file. The save runs at the same point the global-slot save already runs — clean-exit teardown. A connected client still does not write the save file (the shipped Phase 3.5 rule stands).
- **Per-owner restore on boot.** A single-player or host process restores the local player's saved per-owner values at the same lifecycle point global slots restore — after the first successful mod-init commit. The local player's `PlayerClaimId` from player options keys the lookup.
- **Join seed.** A connecting client carries its own saved per-owner values in a new app-protocol message sent during the content-parity stage. The host applies them to the client's newly admitted seat, gated by declaration compatibility. The join seed is the mechanism that makes a player's progress portable across hosts.
- **Capture at seat release.** Before `clear_released_seat_slot_values` drops a departing seat's per-owner entries, the host snapshots them keyed by that seat's `PlayerClaimId`. The snapshot is held in memory for the session's lifetime and written to disk at clean-exit save alongside the still-live seats' values. A guest whose seat is released mid-session still has their progress saved by the host.
- **Unlocking `perOwner + persist`.** Remove the `store_bridge.rs` load error that rejects the combination, and update the SDK typedefs to allow it.
- **On-disk format extension.** The `state.json` format gains a `per_owner` section keyed by durable slot key → player identity → value. The format version bumps from 2 to 3; version 2 files load normally (they contain no per-owner entries).
- **Reference persistence in the dev mod.** The dev mod's per-player XP slot gains `persist: true` and the walkthrough notes session-crossing behavior.

### Out of scope

- **Account identity / cloud sync.** `PlayerClaimId` is device-local. An account-scoped identity would need its own spec; this keying migrates cleanly to it (swap the key, carry the values).
- **Client-side save file writes.** A connected client does not write `state.json`. Its per-owner values reach disk only through the host's save (capture at release + clean-exit) or through its own single-player/host session later via the join seed round-trip.
- **Conflict resolution across hosts.** Two hosts saving different values for the same player and slot produce two save files with different `per_owner` entries. The player's own device holds the authoritative copy via join-seed round-trip; the host's copy is a courtesy backup, not a merge candidate.
- **Save-on-change / periodic autosave.** The save path runs at clean-exit only, matching global-slot persistence. Abnormal termination may lose changes, as the existing persistence contract already states.
- **Partial restore of per-owner values for a late joiner.** A join seed carries the client's full per-owner snapshot; there is no per-slot or per-store selective restore.
- **Per-owner `onStateCrossing`.** Deferred by the currency spec to its own future spec.
- **Exposing `PlayerClaimId` to scripts.** The identity is an engine-side key, never surfaced in the SDK.

## Direction

**Problem.** The persistence layer (`state_persistence.rs`) reads and writes only `record.value` — the single scalar projection per slot. Per-owner slots store their values in `per_seat_values`, a map from `Seat` to value, which the save path never touches. And `Seat` is a session-scoped monotonic integer — it has no meaning outside the session that minted it. Both gaps must close: the save/restore path must reach per-seat storage, and the saved entries must be keyed by a durable cross-session identity instead of by `Seat`.

**Prior commitments.**
- *The guard was placed for this spec.* The currency spec's `perOwner + persist` load error in `store_bridge.rs` names this spec by feature id and explicitly reserves the combination for it to unlock.
- *A connected client does not write the save file.* `should_save_persisted_state` returns false for a connected client. This spec preserves that rule: the host saves on behalf of its guests (capture at release), and a guest's own device receives its values via the join-seed round-trip — the guest saves only when it next runs as host or single-player.
- *The capture hook was named.* The currency spec identified `clear_released_seat_slot_values` as the site where this spec would capture a departing seat's values before they are dropped.
- *`PlayerClaimId` already flows at admission.* A 16-byte device-local identity is generated once per device, persisted in player options, encoded into the `ConnectClaim`, and decoded by the host at `admit_or_reclaim`. This spec reads it — it mints no new identity.
- *The durable-key ledger already keys saved values.* `identity.json` maps authored slot names to opaque durable keys. Per-owner saves extend this: `(durable_key, player_identity)` is the compound key, so a slot rename still preserves saved per-owner values the same way it preserves global ones.
- **Divergence, named:** the on-disk format version bumps from 2 to 3. Version-2 documents still load (they have no `per_owner` section); a version-3 document loaded by an older engine without per-owner support is rejected by the existing version check, which is the correct degradation — an older engine cannot restore values it doesn't understand, and rejecting the file leaves defaults active, matching the shipped contract.

**Alternatives rejected.**
- *Host saves all players' values in its own save file; no join seed.* Simpler, but a player's progress is locked to one host's save file. A different host or the player's own single-player session can't see it. The join seed makes progress portable.
- *Client writes its own save file at disconnect.* Reverses the shipped Phase 3.5 rule and introduces a client-side write path for host-authoritative values. The host already has the values — let it save, and let the join seed carry them back.
- *A new cross-session identity separate from `PlayerClaimId`.* `PlayerClaimId` is already generated, persisted, and flowing through admission. Minting a second identity for the same purpose is waste.

## Decisions

- **Per-owner values are keyed on disk by `PlayerClaimId`, hex-encoded.** A 16-byte identity becomes a 32-character hex string as the JSON key. Hex is deterministic, human-readable in the save file, and avoids base64 padding ambiguity. The local player's identity comes from `player_options.player_id`; a guest's identity comes from its `ConnectClaim.player_id` stored on the seat.
- **The save document gains a `per_owner` section.** Structure: `{ version: 3, slots: { … }, per_owner: { durable_key: { hex_player_id: value, … }, … } }`. The `slots` section is unchanged — global slots save exactly as today. `per_owner` is a nested map: outer key is the slot's durable key (from `identity.json`), inner key is the hex-encoded `PlayerClaimId`, value is the persisted value in the same encoding global slots use (`PersistedValue`).
- **Version 2 files load without error.** An absent `per_owner` section is equivalent to an empty one — no per-owner values to restore. A version-3 file loaded by an older engine is rejected by the version check, leaving defaults active.
- **Capture happens at the `clear_released_seat_slot_values` call site, before the clear.** A new `capture_released_seat_slot_values` function reads each released seat's per-owner entries from every `perOwner + persist` slot, keyed by the seat's `PlayerClaimId` (read from the seat table's stored claim), and stores them in a session-lifetime map (`CapturedPerOwnerState`). The clear then proceeds as today. At clean-exit save, the captured map is merged with the still-live seats' values.
- **A seat with no `PlayerClaimId` (anonymous, no valid claim) does not save.** The capture skips seats whose stored claim is `None` — matching the existing contract that an anonymous client cannot reclaim a seat.
- **The host's own seat (`Seat(0)`) uses the local player's `PlayerClaimId`.** `Seat(0)` has no `ConnectClaim` — it is never admitted through the admission path. Its identity comes from `player_options.player_id` directly, the same value the local player uses in single-player.
- **The join seed is a single app-protocol message.** A new `ClientControlMessage::JoinSeed { slots: BTreeMap<String, PersistedValue> }` sent by the client after content-parity agreement, before participating. The keys are durable slot keys from `identity.json`; values are `PersistedValue`-encoded. The host validates each entry against declared schemas (type match, range, persist flag, per-owner cardinality) and applies valid entries to the client's newly minted seat. Invalid entries warn and are skipped — they do not reject the join.
- **The join seed carries only `perOwner + persist` slots.** A global `persist` slot is session-scoped from the host's save file; a per-owner non-persist slot is runtime-only. The intersection — per-owner and persistent — is the join seed's payload.
- **The client loads its own save file to build the join seed.** The client's `state_persistence::load_persisted_state` runs at boot (as today for global slots). When connecting, the client extracts its own per-owner entries from the loaded document (keyed by its own `PlayerClaimId`) and sends them as the join seed. A client with no save file sends an empty seed — all per-owner slots start at defaults.
- **The join seed is applied after seat assignment, before `levelLoad`.** The host applies the seed to the client's seat in `admit_or_reclaim`'s success path, after the seat is minted or reclaimed but before the client participates in any level fire. A reclaimed seat whose per-owner values survived the hold window keeps those values — the join seed does not overwrite a held seat's live values, only a freshly minted seat's defaults.
- **A reclaim within the hold window does not apply the join seed.** The seat's per-owner values are already live from the previous connection. Overwriting them with the join seed (which may be stale — saved at the client's last clean exit, not at disconnect) would lose in-session progress. The join seed applies only to a fresh seat.
- **Single-player restore uses the same per-owner restore path as co-op.** The local player's `PlayerClaimId` keys the lookup in the `per_owner` section. The restore runs alongside global-slot restore in `overlay_persisted_state`, extended to write per-owner entries into the local seat's per-seat storage.
- **The `PersistedState` struct gains a `per_owner` field.** `per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>` — outer key is durable slot key, inner key is hex player identity. `serde(default)` so version-2 documents deserialize cleanly.
- **`collect_persisted_state` walks per-seat storage for `perOwner + persist` slots.** For each such slot, it reads every seat's per-owner entry (live seats) and merges in the captured entries (released seats), all keyed by hex `PlayerClaimId`. The global-slot collection path is unchanged.
- **`overlay_persisted_state` writes per-owner entries into the local seat's per-seat storage.** It reads the `per_owner` section, finds entries matching the local player's `PlayerClaimId`, validates type/range against declared schemas, and calls `set_per_seat_value` on the local seat. Entries for other players are not restored in single-player — they are preserved in the save file for a future co-op session or join seed, but not loaded into storage.
- **The join seed message is idempotent.** Receiving a second join seed from the same client (e.g., a reconnect that missed the hold window) overwrites the first — same as a fresh seat assignment. Within the hold window, the seed is not applied, so a reconnect there is naturally idempotent.
- **No wire version bump for the join seed.** The join seed is a new `ClientControlMessage` variant, which is an extensible enum. An older host that does not recognize the variant ignores it (the shipped unknown-variant handling warns and skips). An older client that does not send a seed simply starts at defaults — the feature degrades to the pre-persistence behavior.
- **`main.rs` is past the size guidance (~10,700 lines).** The persistence changes (capture, merge, restore extension) are localized to `state_persistence.rs` and the thin call-site wiring in `main.rs`. No split-first task — the extensions are small and touch existing call sites, not new modules.

## Acceptance criteria

- [ ] A `perOwner + persist` slot declaration is accepted at mod-init; the combination that was previously rejected now loads.
- [ ] In single-player, a per-owner slot's value survives engine exit and restart — the restored value matches what was saved.
- [ ] In single-player, a per-owner slot's value survives a level transition within the same session (already shipped by the currency spec; this AC confirms persistence does not regress it).
- [ ] A global persistent slot saves and restores identically to today — the per-owner extension does not alter global-slot behavior. (**Review gate** for byte-identical global-slot save documents when no per-owner slots are declared.)
- [ ] A version-2 `state.json` loads without error under the new format; per-owner slots start at declared defaults. A version-3 file with per-owner entries loads and restores those entries.
- [ ] The save document's `per_owner` section is keyed by durable slot key and hex-encoded `PlayerClaimId`; a slot rename (durable key preserved) restores the renamed slot's per-owner values.
- [ ] In co-op, the host captures a departing guest's per-owner values at seat release (both expiry and stale-duplicate-reclaim paths) and includes them in the clean-exit save.
- [ ] A seat with no valid `PlayerClaimId` (anonymous connection) does not contribute per-owner entries to the save document.
- [ ] In co-op, a connecting client sends a join seed carrying its saved per-owner values. The host applies valid entries to the client's freshly minted seat; invalid entries (type mismatch, undeclared slot, non-persist slot) warn and are skipped.
- [ ] A client with no save file sends an empty join seed; all per-owner slots start at their declared defaults.
- [ ] A reclaim within the hold window does not apply the join seed — the seat's live per-owner values are preserved from the previous connection.
- [ ] After a join-seed apply, the client's HUD shows the restored per-owner values (owner-private replication delivers the applied values).
- [ ] A host that does not recognize the join seed message (older engine) ignores it; the client starts at defaults. A client that does not send a seed (older engine) starts at defaults on the host.
- [ ] Abnormal termination may lose unsaved per-owner changes, matching the existing persistence contract.
- [ ] The dev mod's per-player XP slot declares `persist: true`, and XP survives across single-player sessions.

## Tasks

### Task 1: Unlock `perOwner + persist` and extend the on-disk format

Remove the `perOwner + persist` rejection in `store_bridge.rs` (the `if per_owner && persist { return Err(…) }` guard). Update the SDK typedefs in both TypeScript (`expected.d.ts`) and Luau (`expected.d.luau`) to allow `persist?: boolean` on `perOwner: true` slot variants — the TypeScript `persist?: never` constraint on those union arms becomes `persist?: boolean` (Luau equivalent: `persist: nil?` becomes `persist: boolean?`). The remaining combination rejections (`perOwner + accumulate`, `perOwner + network: "shared"`) stay in place.

Extend `PersistedState` with a `per_owner: BTreeMap<String, BTreeMap<String, PersistedValue>>` field, `serde(default)` and `serde(skip_serializing_if = "BTreeMap::is_empty")`. Outer key: durable slot key. Inner key: hex-encoded `PlayerClaimId`. Bump `CURRENT_STATE_VERSION` from 2 to 3. Update the version check in `overlay_persisted_state`: accept both 2 and 3. A version-2 document has no `per_owner` field; `serde(default)` yields an empty map, so global-slot restore proceeds unchanged and per-owner slots start at defaults. A version-3+ document beyond `CURRENT_STATE_VERSION` is still rejected.

Add a `PlayerClaimId` hex-encode utility — a standalone function in `state_persistence.rs` that takes `[u8; 16]` and returns a `String`, plus its decode inverse for join-seed validation. Import `PlayerClaimId` from `postretro_net::wire` — `postretro` already depends on `postretro-net`.

Add `CapturedPerOwnerState`: a session-lifetime struct holding `BTreeMap<String, BTreeMap<String, PersistedValue>>` (same shape as `PersistedState.per_owner`), plus an `insert` method that takes a `PlayerClaimId`, a slot's durable key, and a `PersistedValue`. This struct lives in `state_persistence.rs` and is owned by the session (threaded through `App` the same way `StateStoreLifecycle` is). The capture call site (Task 2) writes into it; the save call site (Task 3) reads from it.

Test: a `PersistedState` with `per_owner` entries round-trips through serialize/deserialize. A version-2 document deserializes with an empty `per_owner`. Hex encode/decode round-trips. The SDK typedef changes compile (the TypeScript and Luau type checkers accept the new union shape).

### Task 2: Capture at seat release and collect per-owner state at save

Wire the capture into the `clear_released_seat_slot_values` call sites. Before the clear, call a new `capture_released_seat_slot_values` function that, for each released seat:
1. Reads the seat's `PlayerClaimId` from the `SeatTable`'s stored claim (`seat_table.claim_for_seat(seat)` — verify this accessor exists or name the path that exposes the stored `ConnectClaim`).
2. If the claim is `None` (anonymous seat), skips capture.
3. For each slot in the slot table that is `perOwner + persist`, reads that seat's per-owner entry via `record.per_seat_value(seat)` and converts it to a `PersistedValue`.
4. Inserts the entry into `CapturedPerOwnerState` under `(hex_player_id, durable_key)`.

The six existing `clear_released_seat_slot_values` call sites in `main.rs` gain the capture call immediately before the clear. The function needs `&SlotTable`, `&SeatTable` (for claim lookup), `Option<&StoreIdentityLedger>` (for durable keys), `&BTreeSet<String>` (committed membership), and `&mut CapturedPerOwnerState`. Thread `CapturedPerOwnerState` from the session through the same path `StateStoreLifecycle` takes.

Extend `collect_persisted_state` to populate the `per_owner` field of `PersistedState`. After the existing global-slot collection loop, add a second pass:
1. For each `perOwner + persist` slot in the table (filter by `is_persisted_per_owner_slot`), iterate live seats' per-owner entries.
2. For each live seat, resolve its `PlayerClaimId` — `Seat(0)` uses the local player's identity (passed as a parameter), other seats use the seat table's stored claim.
3. Convert to `PersistedValue` and insert under `(durable_key, hex_player_id)`.
4. Merge in the `CapturedPerOwnerState` entries. A live seat's value takes precedence over a captured entry for the same `(player_id, slot)` — the live value is newer.

`collect_persisted_state`'s signature gains `local_player_id: Option<[u8; 16]>`, `seat_table: Option<&SeatTable>`, and `captured: &CapturedPerOwnerState`. The function stays in `state_persistence.rs`; the seat table is passed through by the call site in `main.rs`'s `exiting()`. Single-player passes `Seat(0)` as the only live seat and `local_player_id` from player options; co-op host passes all live seats plus the captured map.

Test: capture stores entries keyed by hex player id. Collect merges live and captured entries, with live winning on conflict. A slot without `perOwner` or without `persist` is excluded from the `per_owner` section. An anonymous seat (no claim) produces no captured entries.

### Task 3: Restore per-owner state and clean-exit save

Extend `overlay_persisted_state` to restore per-owner entries. After the existing global-slot overlay loop, add a per-owner pass:
1. For each entry in `persisted.per_owner`, resolve the durable key to an authored slot name via the identity ledger (same reverse lookup the global path uses).
2. Confirm the slot is `perOwner + persist` and writable (not readonly, mod-owned).
3. For each `(hex_player_id, persisted_value)` under that durable key, check if `hex_player_id` matches the local player's identity.
4. If it matches: validate the value against the slot's declared schema (type, range, enum membership — the same `restored_value` function global slots use), then write it into the local seat's per-seat storage via `set_per_seat_value(local_seat, value)`.
5. If it does not match: skip — other players' entries are preserved in the save file but not loaded into storage. (The join seed delivers them when those players connect.)

`overlay_persisted_state`'s signature gains `local_player_id: Option<[u8; 16]>` and `local_seat: Seat`. The call site in `main.rs` passes the player options' `player_id` and `Seat(0)`.

Update the clean-exit save path in `main.rs`'s `exiting()` to pass the new parameters to `collect_persisted_state`. The save path already calls `save_persisted_state` with the collected document — no change needed there, since `PersistedState` now includes the `per_owner` field and serde handles it.

Test: a round-trip save → fresh-table restore recovers per-owner values for the local player. Entries for other player ids are preserved in the save document but not loaded into the local seat's per-seat storage. A `perOwner` slot without `persist` is not restored. A version-2 document with no `per_owner` section restores global slots normally and leaves per-owner slots at defaults. Range clamping, type mismatch, and enum validation apply to per-owner entries the same as global ones.

### Task 4: Join seed — client send and host apply

**Client side.** After content-parity agreement (the point where the client transitions from parity-checking to participating), the client builds and sends a join seed. The seed is assembled from the client's loaded `PersistedState` (already loaded at boot for global-slot restore): extract the `per_owner` section, filter to entries matching the client's own `PlayerClaimId`, and package them as a `ClientControlMessage::JoinSeed { slots }` where `slots: BTreeMap<String, PersistedValue>` maps durable key to value.

Add the `JoinSeed` variant to `ClientControlMessage` in the wire module (`crates/net/src/wire/control.rs`). The variant carries `slots: BTreeMap<String, PersistedValue>`. `PersistedValue` must be importable from the wire crate — either re-export it from a shared crate, or define a parallel `JoinSeedValue` enum in the wire module with the same variants and a conversion. The wire crate (`postretro-net`) does not depend on `postretro` (the binary crate where `PersistedValue` currently lives), so the type must move down or be duplicated. Prefer moving `PersistedValue` (and its `PersistedState` container) to `postretro-scripting-core` or defining a wire-level `JoinSeedValue` in `postretro-net` — evaluate the crate graph to find the lowest common ancestor. The key point: the join seed must not introduce a dependency cycle.

**Host side.** When the host receives `ClientControlMessage::JoinSeed`, it applies the entries to the sending client's seat:
1. Look up the client's seat by connection id (the message arrives on a known connection).
2. If the seat was **reclaimed** within the hold window (the seat already has live per-owner values), skip the entire seed with a log note — the live values take precedence.
3. For each `(durable_key, value)` in the seed:
   a. Resolve the durable key to an authored slot name via the identity ledger.
   b. Confirm the slot is `perOwner + persist`, writable, and mod-owned.
   c. Validate the value via `restored_value` (type, range, enum).
   d. If valid, write it into the seat's per-seat storage via `set_per_seat_value`.
   e. If invalid, warn and skip.
4. After apply, the seat's per-owner values are live. The next owner-private replication cycle delivers them to the client's HUD.

The host-side handler lives in the netcode module's control-message dispatch, beside the existing `ClientControlMessage` handlers. Thread the slot table, identity ledger, and committed membership through the handler — the same access pattern the existing state-related handlers use.

A client that never sends a `JoinSeed` (older engine, no save file) simply starts at defaults — no timeout or fallback needed. An unrecognized `JoinSeed` on an older host is handled by the shipped unknown-variant skip.

Test: a client with saved per-owner values sends a join seed; the host applies them to the client's seat; the client's HUD shows the restored values. A client with no save file sends an empty seed; values start at defaults. A reclaimed seat (hold window) does not apply the seed. Invalid seed entries (wrong type, out of range, non-persist slot) are skipped with warnings. An older host ignores the message; an older client starts at defaults.

### Task 5: Dev mod reference persistence

Update the dev mod's per-player XP slot declaration in `content/dev/scripts/combat-lifecycle.ts` to add `persist: true`. The shared `teamKills` slot stays non-persistent (it is session-scoped by design). Update the dev HUD walkthrough in `content/dev/README.md` (or equivalent) to note that XP now persists across sessions and travels with the player via join seed. Verify single-player XP survives an engine restart.

## Sequencing

**Phase 1 (sequential):** Task 1 — the format extension and `CapturedPerOwnerState` type that every later task consumes.
**Phase 2 (sequential):** Task 2 — capture and collect. Both Task 2 and Task 3 modify `state_persistence.rs` in disjoint functions; sequencing them avoids merge conflicts.
**Phase 3 (sequential):** Task 3 — restore and save-path wiring.
**Phase 4 (sequential):** Task 4 — join seed. Consumes the save/restore infrastructure from Tasks 2–3 and adds the wire message. The client's send path reads from the loaded `PersistedState` (Task 3's restore produces); the host's apply path writes per-seat storage (Task 2's collect reads from).
**Phase 5 (sequential):** Task 5 — reference persistence. Consumes all of it.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A per-owner value saved to disk is keyed by a durable cross-session identity, not by session-scoped `Seat` | Task 1 (hex `PlayerClaimId` keying), Task 2 (capture resolves claim) | Any path that writes per-owner entries using `Seat` as the key instead of `PlayerClaimId` breaks cross-session identity | AC 6 |
| Global-slot persistence is behaviorally unchanged | Task 1 (format extension is additive), Task 2/3 (per-owner passes are separate loops) | A per-owner code path that touches `record.value` instead of `per_seat_values` corrupts global slots | AC 4 |
| A connected client never writes the save file | Shipped Phase 3.5 rule (`should_save_persisted_state`) | Any new save call site that skips the `is_connected_client` check | AC 7 (host saves on behalf), AC 13 (join seed is the client's restore path) |
| Capture happens before clear at every release path | Task 2 (six call sites wired) | A new release path that clears without capturing loses a guest's progress | AC 7 |
| The join seed does not overwrite a held seat's live values | Task 4 (reclaim-within-hold-window skip) | A reclaim path that applies the seed before checking hold status loses in-session progress | AC 11 |
| Version-2 save files load without error | Task 1 (`serde(default)` on `per_owner`, version check accepts 2 and 3) | A version check that rejects 2 breaks existing saves | AC 5 |

## Ordering pins

| Scenario | Ordering | Expected outcome |
|---|---|---|
| Clean-exit save with live and captured seats | `collect_persisted_state` iterates live seats first, then merges captured entries; a live seat's value wins over a captured entry for the same `(player_id, slot)` | The save document reflects the most recent value for each player×slot |
| Client connects, seat minted, join seed arrives | `admit_or_reclaim` mints the seat; the join seed message arrives on the next poll; the host applies entries to the minted seat before any level-load or policy fire | The client's restored values are live before the first `levelLoad` reaction fires |
| Client reconnects within hold window, join seed arrives | `admit_or_reclaim` reclaims the held seat; the join seed message arrives; the host skips apply because the seat was reclaimed, not minted | The seat's live per-owner values (from the previous connection) are preserved; the stale join seed does not overwrite them |
| Client reconnects after hold expiry, join seed arrives | The held seat was released (per-owner values captured and cleared); `admit_or_reclaim` mints a fresh seat; the join seed applies | The client's saved values from the join seed are applied; the captured values from the expired seat are in `CapturedPerOwnerState` for the host's save file |
| Host exits cleanly with a guest still connected | The guest's seat is live; `collect_persisted_state` reads its per-owner entries from the live per-seat storage; the save document includes both the host's and the guest's entries | Both players' per-owner values are saved |
| Host exits cleanly after a guest disconnected within hold | The guest's seat is held (not released); `collect_persisted_state` reads the held seat's per-owner entries from live storage (they were never cleared) | The disconnected guest's values are saved alongside the host's |
| Host exits cleanly after a guest's hold expired | The guest's seat was released; `capture_released_seat_slot_values` stored the entries in `CapturedPerOwnerState`; `collect_persisted_state` merges them | The expired guest's values appear in the save document from the captured map |
| Single-player restore with per-owner entries for multiple players | `overlay_persisted_state` restores only entries matching the local player's `PlayerClaimId` into `Seat(0)` per-seat storage; other players' entries are left in the save document | Only the local player's values are live; others' entries are preserved for future co-op or join seed |
| Abnormal termination | No save runs | Per-owner changes since last clean exit are lost, matching the global-slot contract |

## Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| per-owner persistence (declaration) | `SlotSchema.persist` + `SlotSchema.per_owner` (both true) | slot declaration key `persist` alongside `perOwner` | `{ perOwner: true, persist: true }` | same |
| player identity (save key) | `PlayerClaimId([u8; 16])` → hex `String` | hex-encoded 32-char string as JSON key in `per_owner` section | — (not author-facing) | — |
| join seed message | `ClientControlMessage::JoinSeed { slots }` | bitcode-encoded `BTreeMap<String, JoinSeedValue>` | — (not author-facing) | — |
| on-disk format | `PersistedState { version: 3, slots, per_owner }` | JSON with `per_owner: { durable_key: { hex_id: value } }` | — (not author-facing) | — |

## Open questions

- **Join seed size limit.** The join seed travels as an app-protocol message. A mod with many `perOwner + persist` slots could produce a large seed. Should there be a byte-size cap, or is the existing message framing sufficient? The renetcode user-data field is fixed-width (256 bytes) but the join seed is a separate app-protocol message, not connection-token data, so it is bounded by the reliable channel's message size limit instead.
- **Multi-mod persistence.** The current save format is per-mod (`state.json` scoped by mod id). The join seed must carry entries from the same mod the host is running. If the client's save file is from a different mod, the durable keys won't match and the seed is effectively empty. This is correct behavior — a player's progress is mod-scoped — but worth noting.
