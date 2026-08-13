# Research — Per-Player Persistence

Findings behind the spec's decisions and code-grounding for its claims.

## Code grounding

| Claim | Source |
|---|---|
| `perOwner + persist` is rejected at mod-init with a named diagnostic | `crates/scripting-core/src/store_bridge.rs` lines 428–433 |
| The existing save/restore reads only `record.value`, never `per_seat_values` | `crates/postretro/src/scripting/state_persistence.rs` — `collect_persisted_state` reads `record.value.as_ref()` (line 109); `overlay_persisted_state` writes via `record.write_value` (line 199) |
| `is_persisted_mod_slot` checks `persist`, `readonly`, and `ownership` but not `per_owner` | `state_persistence.rs` lines 247–251 |
| `CURRENT_STATE_VERSION` is 2 | `state_persistence.rs` line 17 |
| The version check rejects documents with a version other than `CURRENT_STATE_VERSION` | `overlay_persisted_state` lines 151–156 |
| `Seat` is `Copy`, a newtype over `u16` | `crates/foundation/src/seat.rs` |
| `PlayerClaimId` is `[u8; 16]`, carried in `ConnectClaim` | `crates/net/src/wire/control.rs` lines 22, 32–35 |
| `player_options.player_id` is an `Option<[u8; 16]>` generated once per device and persisted | `context/lib/player_options.md` line 30; `crates/postretro/src/session/mod.rs` lines 423–424 |
| A connected client does not write the save file | `main.rs` line 1445: `should_save_persisted_state(can_save, is_connected_client) = can_save && !is_connected_client` |
| `clear_released_seat_slot_values` is called at six sites in `main.rs` | Lines 4049, 4124, 4147, 5460, 5630, 5641 — after `finish_host_poll` and after admission |
| The currency spec named this call site as the capture hook | `context/plans/done/E16--per-player-currency/index.md` Task 1, final paragraph |
| `SlotRecord` exposes `per_seat_value(seat)`, `set_per_seat_value(seat, value)`, `clear_per_seat_value(seat)` | `crates/entities/src/slot_table.rs` |
| `SlotTable::clear_per_seat_values(seat)` iterates all slots | Same file |
| `SeatTable::admit_or_reclaim` takes `ConnectClaim` and stores it | `crates/postretro/src/netcode/seat.rs` line 209 |
| An anonymous client (no valid claim) cannot reclaim a seat | Same file, line 282 |
| `PersistedValue` has five variants: Boolean, Number, String, Array, Unsupported | `state_persistence.rs` lines 63–76 |
| The save document is JSON: `{ version, slots: { durable_key: value } }` | `state_persistence.rs` — `PersistedState` struct, lines 62–66 |
| The identity ledger maps authored slot names to opaque durable keys | `context/lib/scripting.md` §5; `crates/scripting-core/src/store_identity.rs` |
| `main.rs` is ~10,700 lines | `wc -l` output |
| The SDK typedefs enforce `persist?: never` on `perOwner: true` variants | Generated `expected.d.ts` and `expected.d.luau` |

## Seat claim accessibility

The `SeatTable` stores a `ConnectClaim` per seat (set at admission). Verify whether there is a public accessor to read a seat's stored claim by player id:

- `admit_or_reclaim` stores the claim internally (`self.seats[idx].claim = incoming_claim`).
- Need to verify: is there a `claim_for_seat(seat) -> Option<&ConnectClaim>` accessor? If not, Task 2 must add one or thread the claim through the release path.
- The release path (`release_seat`) already surfaces the released `Seat` values. The capture function needs the `PlayerClaimId` for those seats, which means either the seat table exposes claims, or the release path surfaces them alongside the seat values.

This is a plumbing detail for Task 2 — the task paragraph names it as an open verification.

## Crate graph for `PersistedValue` in the wire module

The join seed carries `PersistedValue`-shaped data across the wire. Current dependency direction:

- `postretro-net` (wire crate) does NOT depend on `postretro` (binary crate) or `postretro-scripting-core`.
- `postretro` depends on both `postretro-net` and `postretro-scripting-core`.
- `postretro-scripting-core` does NOT depend on `postretro-net`.

Options for the join seed's value type:
1. **Define `JoinSeedValue` in `postretro-net`.** A parallel enum with the same variants as `PersistedValue`. Convert at the call site. Pros: no new crate edge. Cons: two identical enums maintained in parallel.
2. **Move `PersistedValue` to `postretro-scripting-core`.** Both `postretro-net` and `postretro` could then use it. But this adds `postretro-scripting-core` as a dependency of `postretro-net`, which is not currently the case.
3. **Move `PersistedValue` to `postretro-foundation`.** Both crates depend on it. But `PersistedValue` is a persistence concern, not a foundation type.
4. **Define `JoinSeedValue` in `postretro-net` with a serde-compatible shape.** The host converts from `JoinSeedValue` to the internal representation at apply time.

Option 1 or 4 (effectively the same) is the least disruptive. The join seed is a wire concern; the wire module defines its own types and the binary crate converts at boundaries.

## Shape changes from the currency spec's deferred design

The currency spec's research.md documents that the "second shape" (device-local persistence + join seed) was deemed under-scoped on two counts:
1. Identity lifetime — the player id used was session-scoped.
2. Persistence reversed a shipped rule (client save) without naming it.

This spec addresses both:
1. Uses `PlayerClaimId`, a device-scoped identity that outlives sessions.
2. Preserves the client-no-save rule; the host saves on behalf, and the join seed carries values back.

The currency spec's concern that "a client-to-host state write is a stated Phase 3.5 non-goal" applies to runtime state writes, not to a one-time join-seed apply at admission. The join seed is host-validated and host-applied — the client asserts values, the host decides whether to accept them, matching the authority model.
