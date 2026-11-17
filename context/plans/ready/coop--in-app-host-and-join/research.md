# coop--in-app-host-and-join — research

Read at 7ffced62. Facts that inform the brief but decide nothing on their own.

## Role entry
- `Session::build` is the only role and endpoint construction site. Its order is `parse_net_config`, `NetEndpoint::from_role` (with a `ConnectClaim` built from `player_options.player_id`), `seed_lag_profile_from_environment` in dev builds, `authority_seat_table_or_local_only`, `SessionRole::of`, `published_host_address` (the LAN probe), then `apply_role(RoleBoundState::derive(..))`.
- `apply_role` keeps an existing seat ledger (`get_or_insert_with`) and drops it for a client. SP and host both keep only `Seat(0)`.
- `install_network_mod_content` (startup/lifecycle_net.rs) is a no-op without an endpoint, and it runs only at mod init and on staged commit.
- Install-time keyed state (lifecycle_install.rs) is computed when a level installs, so a backdrop installed in single-player has none of it:
  - the `suppress = is_connected_client()` gate;
  - `set_join_seed`, `set_level_parity`, `set_relevel_catalog_id`;
  - role-keyed model-upload prediction (`level_load/inputs.rs`).
  A relevel reloads a backdrop as the host's level (`boot_sequence.md` §1), so a joining client discards that backdrop.
- Prediction, interpolation, time sync and `session_status` are fresh inside the `Client` variant of `from_role`.
- `App::leave_session` branches on Splash and FirstLaunchHold, and otherwise calls `navigate_to_frontend`.

## Saves
- `session.persisted_state` is the snapshot read at mod init. Only its per-owner parts refresh later (`retain_saved_per_owner_state`). World and global slots save only at a clean exit, gated by `StateStoreLifecycle::can_save`.
- Leave restores through `restore_store_after_leave` → `restore_store_after_session(.., session.persisted_state, ..)`, plus faction sentiment from the same snapshot. Without a save at join, SP progress made after boot is lost on leave.
- The join seed comes from `join_seed_from_persisted_state(session.persisted_state)` at install, so without a rebase it is the boot-time character.
- At mod init a client skips the per-owner and faction-sentiment overlay (`overlay_persisted_state`). A runtime client already holds both live, and the host's replication overwrites the replicated ones.

## Admission and transport
- `NetServer::process_control_messages` (crates/net/src/transport.rs) runs `validate_handshake`, then the mod-id check, then `slots.admit`. A mismatch goes to `NetServer::reject`, which sends `DivergenceReason::Closing(cause)` reliably and disconnects once Control is fully acknowledged (`apply_pending_disconnects`).
- The host never sends an admitted message. The client queues `Admission`, `Parity`, `JoinSeed` and `Revealed` without waiting (`NetClient::queue_control_messages`), and the host stores parity declarations before admission.
- `ClosingCause` holds `Protocol`, `ModId` and `SessionEnded`. The client match in main.rs falls any other closing cause to `leave_session(LeaveCause::Refused)`.
- The version constants are `PROTOCOL_ID` and `WIRE_VERSION` in handshake.rs. Appending `SessionEnded` moved only `PROTOCOL_ID`.
- Both sides use netcode `Unsecure`. The connect-token `user_data` (256 bytes) carries `ConnectClaim` (`encode_connect_claim`). Unsecure tokens use a public key, so `user_data` is effectively plaintext. That is from renetcode knowledge; the renet sources are not on disk to confirm it.
- Retries: `retry_unanswered_connect` covers only unanswered connect attempts (`CONNECT_ATTEMPTS`), and every `ClosingCause` is terminal.
- `blake3` is a workspace dependency, but not one of `crates/net`'s. There is no `hmac` or `sha2` in the lock file.

## UI
- There is no text-field widget. Text entry pushes the engine keyboard with `openTextEntry`. Edits route to the active tree's `textEntryTarget` (`resolve_text_entry_intents`). The dev mod's key reactions append to `ui.textEntry` explicitly (`content/dev/scripts/arena-lights.ts`).
- The keyboard has digits, letters, space, delete and done. It has no `.` or `:`.
- No engine notice surface exists. Dialogs are authored (`showDialog`), and `Announce` is the live-region widget.
- Reserved `ui.*` actions (crates/ui/src/actions.rs) include no host or join action.
- The existing session slots are `hosting`, `hostAddress`, `openSeats` and `playerCounts.*`, all readonly. The dev frontend already shows `HOSTING ON {}`.
- `PlayerOptions` has no address field.

## Ordering pins

| Id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | `hostSession()` then `joinSession()` in one reaction; `joinSession()` twice in one reaction | Both drain in one frame-end drain, in authored order | The first verb takes effect inside the drain. The second sees the new role and is refused with a warning: no second save or endpoint, no `endCause` clear, no password consumed. |
| P2 | On a client, one reaction fires `leaveSession()` then `joinSession()` | Leave lands in the frame-end drain; the join comes later in the same drain | The join survives the post-leave drop as a session verb and defers until the left level unloads, then joins. Other steps after the leave still drop at debug. |
| P3 | A frontend backdrop load is in flight; `joinSession()` | Backdrop dispatch with single-player inputs → join | The join cancels the backdrop load. The host's level loads afterwards with client-role install state and model prediction. No single-player-dispatched product serves as the host's level. |
| P4 | `loadLevel(x)` then `joinSession()` in one reaction; or Join pressed during x's load | Load queued or in flight → entry check | Refused with a warning. Permitted once only a backdrop load remains. |
| P5 | A leave or end lands and the frontend menu shows; Host or Join pressed before the left level unloads, or before an ended host's closing transport drops | Menu shown → press → unload or close still pending | The press waits, publishing `connecting`, and runs once both finish. A second press meanwhile is refused. |
| P6 | Password host; client with the right password | Admission, Parity, JoinSeed and Revealed arrive in one batch → challenge sent → proof arrives at least one round trip later → admit | Admission at proof resends the installed relevel and evaluates the stored parity and reveal. The buffered join seed applies at participation. The client re-declares nothing. |
| P7 | N clients pending on one password host; a proof captured on one connection and replayed on another; a proof with no challenge outstanding | Challenges go out per slot; proofs arrive in any order | Each pending slot gets its own nonce. A replayed proof refuses with `refusedPassword`. A proof with no challenge outstanding changes no slot state. |
| P8 | Password host; client stays connected and sends no proof | Challenge → keepalives only | Refused with `refusedPassword` when the host-side bound elapses. The slot never reaches admitted. |
| P9 | Password host; client on the wrong mod with a wrong password | Admission arrives → mod check | Refused with `refusedMod`. No challenge is sent. |
| P10 | Hosting with password p; `session.password` written to q mid-run | Write → next client's challenge | Proofs from p still admit and proofs from q refuse, until the next hosting. |
| P11 | Host refuses, and the transport disconnect lands in the same client update | Control drain → connection-end check | `endCause` is the refusal's value, never `hostLost` or `connectFailed`. |
| P12 | A join opens the transport, then is refused (mod or password) | Transport established → refusal | The address is not remembered, and `session.connection` never shows `waiting`. |
| P13 | `joinSession()` with a malformed address | Clear `endCause` → parse | `badAddress`. No save, the connection stays `offline`, and the password is kept. |
| P14 | Boot `--host` fails to bind; boot `--connect` gets a malformed address; a host runs its own `endSession` | Boot role build, or the end | `hostFailed`, `badAddress`, and no cause, respectively. |
| P15 | Host admits a joiner, ends at its frontend, its closing transport drops, then it hosts again with no session between | End (ledger kept with seat 0) → host | The second hosting publishes a new session id. Its first joiner gets the first remote seat id. Seat 0 keeps its carried state. |
| P16 | A press waits on an unload or close; meanwhile the menu edits the address or password, or starts a level load or navigation | Press → edit or load → window closes | The press runs with what it captured; edits do nothing. A load or navigation cancels it and publishes `offline`. |
| P17 | A join is pending (connecting or challenged); the host is killed or ends its session | Connect → host gone → no admission | `connectFailed`. The address is not remembered. |
| P18 | Host pressed while an ended host's close is pending; separately, the deferred host's bind or mint fails | Press → close drops → host build | `session.connection` reads `connecting` while the press waits, then `offline` as `session.hosting` publishes. A failed build returns it to `offline` with `hostFailed` and leaves the seat ledger untouched. An undeferred host press never leaves `offline`. |
| P19 | Admitted client held for a mod digest, a level digest or identity, a level it lacks, or the host at its frontend | Holding notice → slot | `waitReason` is `modDiffers`, `levelDiffers`, `levelDiffers` and `host`, respectively. It empties at participation. |
| P20 | Password lifetime across outcomes | Verb → outcome | Cleared on hosting, admission and `refusedPassword`. Kept on `connectFailed`, `hostFailed` and `badAddress`. |
| P21 | Password host; a held seat's client reconnects with the right password; the hold deadline falls between its Admission and its proof | Admission → challenge → hold expires → proof → admit | The seat is judged at the proof, as admission is. A hold that expired while the proof was in flight is not reclaimed, and the client gets a fresh seat. A proof landing on the deadline poll reclaims, as an Admission does today. |
| P22 | Password host; the proof arrives in the poll the bound elapses; separately, the host ends or changes level while the challenge is outstanding | Control drain (proof) → bound check; or end / relevel → proof | A proof drained in the bound's poll admits. An end refuses with `sessionEnded`, never `refusedPassword`. A level change does not restart the bound, and a proof after it admits into the newly installed level. |
| P23 | A holding notice and a participation marker in one client update; separately, an admission and a session-ended cause in one update | Control drain in order | The slot publishes each phase in order, `waiting` included, even for zero frames. The address is remembered because the join was admitted, and `endCause` is `sessionEnded`. |

## Notes for the executor
- `parse_net_config` returns one untyped `NetArgError` for every flag fault. Boot can set `badAddress` only for a malformed `--connect` once the error says which flag failed.
- Admission is first visible to the client as the `Relevel` or `Holding(HostLevelAbsent)` notice sent at admission, not the roster, which `finish_host_poll` sends later.
