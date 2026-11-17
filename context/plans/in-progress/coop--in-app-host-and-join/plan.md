# coop--in-app-host-and-join — plan of record

mode: resumable
status: proposed
read at: 682a264a

Brief read at 7ffced62. Since then only `crates/netcode/src/command_queue.rs` and `crates/sim/src/sim/{host_movement,mod}.rs` changed, none cited. Every Decision and Path symbol was re-read at this commit. No Decision premise is false.

## Corrections
- Research: "the host stores parity declarations before admission" → `NetServer::process_control_messages` (crates/net/src/transport.rs) also accepts `JoinSeed` (pushed to `join_seeds`) and `Revealed` (into `revealed_declarations`) from a `Pending` slot without a state check. Today the mod-id verdict lands in the same batch as `Admission`, so nothing leaks. With a password the verdict waits for a proof. Planned around: a pending slot with an outstanding challenge buffers its `Parity`, `JoinSeed` and `Revealed` in transport. On admission it applies them in arrival order. On refusal it drops them (AC P6, "refused … applies no parity, join seed or reveal").
- Path: "`--connect` widens to match" → `parse_net_config` (crates/netcode/src/lib.rs) parses a full `SocketAddr`, so the port is required today. Planned around: one address parser in netcode (IPv4, optional port defaulting to `DEFAULT_HOST_PORT`, hostname and IPv6 refused), shared by `--connect` and `joinSession`.
- Path: "Settle how each field reaches its own slot" → `openTextEntry(onCommit?)` (sdk/lib/ui/reactions.ts) always pushes `core/ui/keyboard.json`, whose envelope fixes `text_entry_target: "ui.textEntry"`, and `resolve_text_entry_intents` edits only the active tree's target. Proposed approach (**owner skim**): `openTextEntry` gains an optional target slot that overrides the keyboard's `text_entry_target` for that push, so hardware keys and on-screen keys both write `session.joinAddress` or `session.password` directly. The password then never passes through `ui.textEntry`. This is an additive SDK parameter the boundary inventory does not list. The fallback needs no new surface: each field's `onCommit` copies `ui.textEntry` into its slot with `setState` and then `clearText`. The cost is that the password sits in `ui.textEntry` while it is typed.
- Path: "a new ledger operation that keeps seat 0's carry" → confirmed. `SeatTable`'s `session_id` and `next_seat` are private, and `apply_role` keeps any existing table (`get_or_insert_with`). Planned: `SeatTable::remint_for_hosting() -> Result<(), getrandom::Error>`. It draws the new id first, and only on success resets the id, the counter, the hold state and the roster bookkeeping. It keeps `carried[Seat(0)]` and seat 0's pawn binding. On failure the table is untouched.

## Delegated answers
- Host-side bound on an unanswered challenge — **5 s** on the transport clock, measured from when the challenge was sent. One round trip is under 1 s even under the dev lag profile with loss, and 5 s sits well inside a seat hold. A proof drained in the poll where the bound elapses still admits (P22).
- Remembered address field and options mapping — `PlayerOptions::last_join_address: Option<String>` (TOML key `last_join_address`, default absent). It seeds `session.joinAddress` at session build, and the engine writes it on admission. It is not an options-bridge menu option, because no menu edits it except the Join field itself.

## AC-to-proof

Automated rows run as `cargo test` loopback tests over real transports, as the leave brief's membership tests do. Each row's test lives beside its owning module.

| AC | Proof | Status |
|---|---|---|
| E1 SP at frontend hosts; slots publish; client waits; next level promotes it | `in_app_host_*` loopback test | achievable as stated |
| E2 SP joins a host in a level, with and without a same-map backdrop; participates at hinted spawn | `in_app_join_*` loopback test, two variants | achievable as stated |
| E3 Join during backdrop load cancels it; client-role install state and model prediction (P3) | loopback test, asserts install-time keyed state | achievable as stated |
| E4 Host, end, join elsewhere, leave, host again in one process | loopback cycle test | achievable as stated |
| E5 Re-host mints new session id and seat counter; seat 0 keeps carry (P15) | `SeatTable::remint_for_hosting` unit + loopback | achievable as stated |
| E6 Refusal matrix with no state change, each paired with a permitted case (P4) | table-driven test over role and load states | achievable as stated |
| E7 Press during pending unload or close publishes `connecting`, runs after; second press refused (P5) | deferral test | achievable as stated |
| E8 Failed mint at host entry: ledger untouched, SP, `hostFailed` | injected-entropy-failure test | achievable as stated |
| E9 `hostSession`+`joinSession` and `joinSession`×2 in one reaction: first wins, one save and one endpoint (P1) | reaction-drain test | achievable as stated |
| E10 `leaveSession` then `joinSession` defers to after unload; other steps still drop at debug (P2) | reaction-drain test with log capture | achievable as stated |
| E11 Waiting press captures address and password; malformed never shows `connecting`; load or navigation cancels to `offline` (P16) | deferral test | achievable as stated |
| E12 Boot `--host`/`--connect` role-bound state equals in-app entry | `RoleBoundState::observe` comparison | achievable as stated |
| E13 One endpoint builder; no other non-test `NetEndpoint::from_role` call | grep gate test | achievable as stated |
| S1 World and character slots on disk once join starts, before first connect packet | join-save ordering test | achievable as stated |
| S2 Join then leave restores world-at-join; join seed carries character-at-join | loopback test | achievable as stated |
| S3 Failed join save still rebases snapshot and warns | injected-write-failure test with log capture | achievable as stated |
| S4 Join then leave: seat-0 carry equals a fresh SP ledger | loopback test | achievable as stated |
| P-a Right password admits | transport test | achievable as stated |
| P-b Pre-proof parity, seed and reveal apply on admission; participates with join-seeded character (P6) | loopback test | achievable as stated |
| P-c Distinct nonces; replayed proof refuses; stray proof changes nothing (P7) | transport test | achievable as stated |
| P-d Wrong mod refused `refusedMod`, no challenge (P9) | transport test | achievable as stated |
| P-e Live write of `session.password` takes effect only at next hosting (P10) | loopback test | achievable as stated |
| P-f Wrong or empty-password proof refuses `refusedPassword`; client lands at frontend | loopback test | achievable as stated |
| P-g Silent client refused within bound (P8) | transport test, advanced clock | achievable as stated |
| P-h Proof on the bound's poll admits; end during challenge sends `sessionEnded`; level change admits into new level (P22) | transport + loopback tests | achievable as stated |
| P-i Password host: reclaim on deadline poll; fresh seat after hold expiry (P21) | seat + transport test | achievable as stated |
| P-j Passwordless host sends no challenge and admits clients with and without a password | transport test | achievable as stated |
| P-k Password never in datagrams, saves, slots or logs; slot cleared once consumed | byte-scan test with log capture, worker-thread record assertion | achievable as stated |
| P-l Password-refused client applies no parity, seed or reveal | transport test | achievable as stated |
| P-m `--host --password p` and `--connect --password p` | CLI parse + loopback test | achievable as stated |
| P-n Wire round-trip and protocol-id tests; protocol id advanced | `crates/net` wire tests | achievable as stated |
| F1 Each end cause on its own path, incl. same-update refusal; `badAddress` cases open no socket; portless dials default; `Protocol` → `connectFailed` (P11) | per-cause tests | achievable as stated |
| F2 `badAddress` join: no save, stays `offline`, keeps password (P13) | facts test | achievable as stated |
| F3 Own `endSession` sets none; boot `--host` failure `hostFailed`; malformed `--connect` `badAddress` (P14) | boot-path tests | achievable as stated |
| F4 `session.connection` full sequence recorded per publish, incl. same-update hold+promote and admission+end (P23) | publish-recording test | achievable as stated |
| F5 Deferred host press `offline`→`connecting`→`offline`; failed build ends `hostFailed`; undeferred never leaves `offline` (P18) | facts test | achievable as stated |
| F6 `endCause` clears at next attempt; own leave sets none; host gone before admission → `connectFailed` (P17) | facts test | achievable as stated |
| F7 `waitReason` mapping from each `HoldingCause` (P19) | mapping unit + loopback | achievable as stated |
| F8 `session.password` clear and keep rules (P20) | facts test | achievable as stated |
| F9 Remembered address seeds after restart; updated only on admission (P12) | options round-trip test | achievable as stated |
| U1 `mask: true` draws one `*` per grapheme; empty for empty | `crates/ui` layout/text test | achievable as stated |
| U2 Keyboard types `.` and `:` | `keyboard_asset` test | achievable as stated |
| U3 Scripting surface example builds in a `content/dev` script | dev-mod script build | achievable as stated |
| U4 Late-joiner catch-up ignores host/join steps silently | `machine_local` test with log capture | achievable as stated |
| U5 Luau `UI.hostSession()`/`UI.joinSession()` produce the same steps | SDK parity test | achievable as stated |
| U6 No host or join `ui.*` action | grep gate test | achievable as stated |
| U7 `networking.md` wording corrected | grep gate test | achievable as stated |
| U8 Dev frontend shows a notice per end cause, list derived from the enum | dev-mod coverage test | achievable as stated |
| M1 Two-machine LAN host and join, gamepad and hardware keyboard | owner, two machines | manual |
| M2 Each notice on a real client, including different engine build | owner, two machines | manual |
| M3 Joining a host at its frontend shows waiting, then plays | owner, in-engine | manual |
| M4 SP progress survives join and leave | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | **Thin slice: runtime join.** Extract the role and endpoint build out of `Session::build` into one function (`session/net_entry.rs`): parse or accept a role, `from_role`, the lag seed, the seat ledger, `SessionRole`, the published address and `apply_role`. Boot calls it. Add `App::join_from_frontend(addr)` beside `startup/session_membership.rs`. It cancels a backdrop load, calls the builder, then `install_network_mod_content`. Loopback against a CLI-style host, with no password, saves or slots. Proves E2, E3, E13, and the boot half of E12. | integrating executor | — | |
| 2 | **Runtime host.** `SeatTable::remint_for_hosting`; `App::host_from_frontend()` through the same builder, `hostFailed` on mint, bind or transport failure. Proves E1, E4, E5, E8, rest of E12. | integrating executor | 1 | |
| 3 | **Join save seam.** Extract the clean-exit save in `main.rs` into one entry point. Join calls it before building the endpoint, then rebases `session.persisted_state` (and so the join seed) on what it wrote, warning on failure. Proves S1–S4. | integrating executor | 1 | |
| 4 | **Verbs, refusals, deferral.** `SystemReactionKind::{HostSession, JoinSession}`, machine-local classification and catch-up ignore, the drain exemption after `leaveSession`, the one-press rule, and the pending-press state that captures address and password and is cancelled by load or navigation. SDK `hostSession()`/`joinSession()` in TS and Luau plus generated declarations. Proves E6, E7, E9–E11, U4, U5. | integrating executor | 2, 3 | |
| 5 | **Session facts.** Catalog `session.joinAddress`, `password`, `connection`, `waitReason`, `endCause`. Shared address parser and `--connect` widening. End-cause publication on every path, boot included. Connection phase from holding notices and participation. `waitReason` from `HoldingCause`. `last_join_address` option. Proves F1–F9 except the password rows. | integrating executor | 4 | |
| 6 | **Password gate.** `crates/net` gains `blake3`. Wire variants appended, `PROTOCOL_ID` advanced. Per-pending-slot nonce, proof check in constant time, the 5 s bound, buffered pre-admission declarations, and interactions with reclaim and level change. `--password`. Client answers from the captured password, and the slot clears per P20. Leak-scan test. Proves P-a–P-n and the password half of F1 and F8. | integrating executor | 5 | |
| 7 | **UI.** `Text` `mask`, keyboard `.` and `:` keys, the text-field target approach (see Corrections), and dev frontend Host and Join submenus with phase, wait reason and per-end-cause notices. Proves U1–U3, U6, U8. The `crates/ui` mask and keyboard part may run as a worktree worker beside task 6. | integrating executor (mask and keyboard: optional worker) | 5 | |
| 8 | **Docs and gates.** `networking.md` §Two-gate, §Role model and §Session-state ledger corrections, plus the mask screen-reader rule in `ui.md` §4.2. Proves U7. The `scripting.md` §10.4 and `player_options.md` updates wait for landing. | integrating executor | 6, 7 | |
