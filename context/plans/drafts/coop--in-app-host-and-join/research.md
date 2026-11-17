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
