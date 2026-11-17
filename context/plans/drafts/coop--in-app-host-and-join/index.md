# coop--in-app-host-and-join

Brief · resumable · Epic 15 · reads: `context/lib/networking.md` §Two-gate handshake, §Admission and content parity, §Leaving and ending a session, §Role model · `context/lib/ui.md` §3, §4 · `context/lib/scripting.md` §5, §10.4 · `context/lib/player_options.md` §2, §4 · `context/lib/boot_sequence.md` §1 · `context/research/coop-session-lifecycle.md` · read at 7ffced62

## Problem
The owner wants players to host and join co-op from an on-screen menu. Basis: a requested capability. Today role comes only from `--host` / `--connect` at launch, and only single-player can follow it (`networking.md` §Role model). No session password exists. Nothing tells a player why a connect failed or a session ended: the leave brief lands a client at its frontend silently and left the notice to this brief. When this is done, a player at the frontend hosts, optionally with a password, or joins by typing an address and password with keyboard or gamepad. They see that they are connecting, then play in the host's level. When they are refused, lose the host, see the session end or fail to connect, the menu says which. Single-player progress made before an in-app join survives the join and the later leave. Hosting or joining again needs no relaunch.

## Decisions
- **Runtime role entry, from the frontend only.** A single-player machine becomes a host or a client after boot, as many times as it likes. This narrows `networking.md` §Role model ("Hosting or joining again needs a relaunch"). The frontend means the frontend menu is up with no played level installed (a backdrop counts as the frontend). Hosting from a running level is a non-goal: it would replay install-only host registrations against a live level (`host_register_own_pawn_after_install`, `host_register_loaded_movers`, `set_relevel_catalog_id`, `publish_net_reveal`), and nothing does that today.
- **One construction path for boot and entry.** Boot `--host` / `--connect` and in-app entry share the endpoint build, network mod-content seeding (`install_network_mod_content`), dev lag seeding and `RoleBoundState::derive` / `apply_role`, so the two cannot drift. This extends the leave brief's one-derivation rule (`done/coop--client-leave-session`) to the entering direction. Entry refuses a local-only seat ledger (`SeatTable::local_only`), whose id must never publish.
- **Refusals.** Entering is refused with a warning off the frontend, in any role but single-player, and while an ended host's closing transport still drains. A refused verb changes no state.
- **The host's frontend is the interim gathering place.** An in-app host stands at its frontend with a live endpoint, and clients wait there held until it starts a level. The leave brief allowed this only for command-line hosts, "until that brief lands" (`done/coop--client-leave-session`), against "the frontend is outside every session" (`networking.md` §Leaving and ending a session). This brief makes it the player-facing flow until the optional-lobby brief gives hosts a lobby scene. Undo is cheap: that brief changes where the host stands, not how entry works.
- **Each hosting is a new hosted run.** Entry mints a fresh session id and seat counter (`done/E15--seat-session-identity-roster`; `networking.md` §Session-state ledger), and seat 0 keeps its carried state, as an ended host keeps it. A kept ledger from an earlier run never carries its id or exhausted seats into the next one.
- **A host failure degrades as at boot.** A bind or transport failure leaves the machine single-player and sets the end cause, the same degrade boot already applies.
- **Join saves first.** Before connecting, a join saves the world and character to disk through the same entry point the clean-exit save uses. The leave-restore snapshot (`session.persisted_state`) and the join seed rebase on what it wrote, so a later leave restores the world as it was at join, not at boot. If the write fails, the snapshot rebases anyway and the failure warns. The character/world save split (roadmap, next) changes that one entry point.
- **Mods own the menus; the engine owns entry, admission and the facts.** `hostSession()` and `joinSession()` are reaction primitives beside `leaveSession()` (`scripting.md` §10.4). The engine adds no `ui.*` action and no engine-drawn dialog.
- **Engine session slots.** Each is client-local and nonpersistent:

  | Slot | Writable | Meaning |
  |---|---|---|
  | `session.joinAddress` | yes | Address the next join dials. Seeded at boot from the remembered address. |
  | `session.password` | yes | Password for the next host or join. Never logged. Cleared once a verb consumes it. |
  | `session.connection` | no | `offline` / `connecting` / `connected`. A host reads `session.hosting`. |
  | `session.endCause` | no | Why the last session or attempt ended (vocabulary below). Empty at boot; cleared when the next host or join starts. |

  The research leaned toward a password never entering a store slot (`coop-session-lifecycle.md` §6). This diverges: text entry writes only slots. The slot never replicates, persists or logs, and it empties on use.
- **End-cause vocabulary.** One value per way a session or attempt ends: `sessionEnded`, `hostLost`, `connectFailed`, `refusedMod`, `refusedVersion`, `refusedPassword`, `badAddress`, `hostFailed`. A client's own leave sets none. Later causes (eviction) append. Values are data; the mod supplies the wording.
- **Address grammar.** Matches `--connect`: IPv4 literal, port optional and defaulting to the host default. No DNS. A malformed address never connects and sets `badAddress`. The last address whose transport connected is remembered in player options (`player_options.md` §2). The password is never remembered.
- **Session password is an engine admission gate.** It is checked after the mod id, before `slots.admit`, by challenge and response. The host sends a fresh nonce, and the client answers with a keyed hash of password and nonce (Wire format). The password never crosses the wire, and connect-token `user_data` is plaintext under netcode `Unsecure`. A wrong or missing answer is a terminal refusal with `ClosingCause::Password`. The client takes the refused-leave path, and the menu retries by reconnecting. A host without a password sends no challenge and admits anyone, including a client that holds a password. Nothing a client declared before admission applies until it is admitted. The password is fixed for a hosted run, so it passes admission's immutability test (`networking.md` §Admission and content parity). It turns admission from a compatibility gate into an access decision, and `networking.md` §Session-state ledger says so once this lands. This gates access only; identity authentication stays a non-goal (`coop-session-lobby.md` §6).
- **CLI parity.** `--password <text>` sets the password for `--host` and supplies it for `--connect`. Both flags stay developer paths.
- **UI additions.** `Text` gains `mask: true`, which draws one `*` per grapheme of its bound content: ASCII renders in every bundled typeface. The engine on-screen keyboard (`core/ui/keyboard.json`) gains `.` and `:` keys, and the dev mod registers their append reactions as it registers the other keys.
- **First consumer.** The dev frontend (`content/dev/scripts/frontend-menu.ts`) gains Host and Join submenus. Host takes an optional password; Join takes an address and a password. Both show connecting and connected, and a notice per end cause.
- **Docs.** `networking.md` §Two-gate's "encrypted netcode handshake" is false under `Unsecure`, and gate 1 is a `protocol_id` comparison. Correct it with §Role model and §Session-state ledger's "Admission is a compatibility gate, not a trust decision".
- **Non-goals.**
  - Hosting from a running level. Warrant above.
  - Optional lobby and navigation, the join predicate, lifecycle reactions, `evictPlayer`: the lobby briefs and Phase 3.75.
  - Separate character and world save files, level-boundary saves: the saves brief.
  - Retrying a password inside one connection, discovery, relay, NAT traversal, hostnames, IME.
  - AccessKit on text entry: E23.
  - Choosing a host port in-app: `--host <port>` covers it.

### Scripting surface
TypeScript in `sdk/lib/ui/reactions.ts` beside `leaveSession()`, exported from `postretro/ui` and declared in the generated `.d.ts` and `.d.luau`. The Luau mirror uses the same names (`UI.hostSession()`, `UI.joinSession()`).

```typescript
import { defineReaction, getGameState } from "postretro";
import { bindState, hostSession, joinSession, stateEquals, Text } from "postretro/ui";

const { session } = getGameState();

// Host and Join buttons. Each reads session.password; joinSession also reads session.joinAddress.
export const host = defineReaction("frontend.host", hostSession());
export const join = defineReaction("frontend.join", joinSession());

// A password field and a refusal notice.
Text({ content: "", mask: true, bind: bindState(session.password) });
Text({ content: "WRONG PASSWORD", visibleWhen: stateEquals(session.endCause, "refusedPassword") });
```

Both take no arguments and return a `SystemReactionStep` (`{ primitive: "hostSession", args: {} }`, `{ primitive: "joinSession", args: {} }`). Both are machine-local, and both are ignored on a late joiner's catch-up, as `leaveSession` is.

## Acceptance
### Automated
**Entry**
- [ ] A single-player machine at the frontend hosts. `session.hosting` and `session.hostAddress` publish, a client connects and waits, and the host's next level promotes it.
- [ ] A single-player machine at the frontend, with no backdrop and with a backdrop of the host's map installed, joins a host already in a level. It ends participating in that level with its pawn at the hinted spawn.
- [ ] A join started while a backdrop of the host's map is still loading ends participating in the host's level, with client-role install state and model prediction (P3).
- [ ] Host, end, join elsewhere, leave, host again: all in one process, with no relaunch.
- [ ] Host, admit a joiner, end, host again with no session between: the session id differs, the next joiner gets the seat id a fresh session gives, and seat 0 keeps its carried state (P15).
- [ ] The second hosting publishes a session id different from the first, and its first joiner gets a seat id as a fresh session would, while seat 0 keeps its carried state.
- [ ] Entry is refused with a warning, and no state changes (role, endpoint, seat ledger, `session.password`, `session.endCause`, `session.connection`, and no save is written), in each case: a played level installed, already hosting, already a client, a closing transport still draining, and a local-only seat ledger. Each refused case is paired with one that is permitted.
- [ ] One reaction firing `hostSession()` then `joinSession()`, and one firing `joinSession()` twice: the first verb takes effect, the second is refused, and exactly one join save and one endpoint result (P1).
- [ ] On a client, `leaveSession()` then `joinSession()` in one reaction leaves, and the join drops without a warning, a save or an endpoint (P2).
- [ ] Boot `--host` and `--connect` produce the same role-bound state as in-app entry. The same compared fields match.
- [ ] One function builds an endpoint from a role, and boot and in-app entry both call it. No other non-test call of `NetEndpoint::from_role` exists. Grep gate.

**Saves**
- [ ] A world slot and a character slot written in single-player after boot are on disk once a join starts, and before its first connect packet.
- [ ] Join, then leave: the world equals the world at join, not at boot. The join seed carries the character as it was at join.
- [ ] A failed join save still rebases the restore snapshot, and warns.

**Password**
- [ ] The right password admits.
- [ ] A client admitted by password, whose parity, join seed and reveal reached the host before its proof, participates in the host's level with its join-seeded character (P6).
- [ ] Two clients pending on one host receive different nonces, and a proof replayed from one connection onto another refuses with `refusedPassword` (P7).
- [ ] A client on the wrong mod is refused with `refusedMod` and receives no challenge, whatever its password (P9).
- [ ] Writing `session.password` on a live host changes which proof admits only at the next hosting (P10).
- [ ] A wrong password and an empty answer each refuse with `refusedPassword`, and the client lands at its frontend.
- [ ] A host with no password (an empty slot) sends no challenge and admits both a client with no password and a client with one.
- [ ] No password byte appears in any sent datagram, store slot, save file or log line. The test scans captured bytes for the password.
- [ ] A client refused for its password applies no parity, join seed or reveal to the host.
- [ ] `--host --password p` admits a client answering `p` and refuses one answering `q`. `--connect --password p` is admitted by a host holding `p`.
- [ ] Wire round-trip and protocol-id tests cover the new variants, and the protocol id has advanced.

**Facts**
- [ ] Each end cause publishes on its own path, including a refusal whose cause arrives in the same update as the disconnect (P11). `badAddress` is checked against a malformed address, an IPv6 address and a hostname, none of which opens a socket, while an address without a port dials the default.
- [ ] `session.connection` steps `offline` → `connecting` → `connected` → `offline`.
- [ ] `session.endCause` clears when the next attempt starts, and a client's own leave sets none.
- [ ] `session.password` empties once a verb consumes it.
- [ ] The remembered address seeds `session.joinAddress` after a restart. It updates only when a connect establishes.

**UI**
- [ ] `mask: true` draws one `*` per grapheme and an empty string for empty content.
- [ ] The on-screen keyboard types `.` and `:`.
- [ ] The Scripting surface example builds in a `content/dev` script.
- [ ] A late joiner's catch-up ignores `hostSession` and `joinSession` steps without a warning, as it ignores `leaveSession`.
- [ ] `UI.hostSession()` and `UI.joinSession()` in a Luau script produce the same steps as the TypeScript calls.
- [ ] The reserved `ui.*` actions gain no host or join action. Grep gate.
- [ ] The dev frontend shows a notice for every end-cause value. The test derives the list from the end-cause enum.

### Manual
- [ ] Two machines on a LAN: host from the dev menu with a password, then join from the other by gamepad on the on-screen keyboard, and again by hardware keyboard.
- [ ] Each notice shows on a real client: wrong password, host ends, host killed, unreachable address after the retry window, and a wrong-mod host.
- [ ] Play single-player, join in-app, leave: the single-player world shows its pre-join progress.

## Path
- The boot path's role build lives in `Session::build` (`netcode::parse_net_config`, `NetEndpoint::from_role`, `authority_seat_table_or_local_only`, `published_host_address`). Extract it once so entry calls it, then call `App::install_network_mod_content`, which today runs only at mod init and on staged commit.
- The exit save is inline in `main.rs` (`collect_persisted_state`, `collect_persisted_faction_sentiment`, `collect_per_owner_state`, `merge_per_owner_state`, `save_persisted_state`). Extracting it is the join save's seam. `restore_store_after_leave` and `join_seed_from_persisted_state` read `session.persisted_state`.
- Admission is decided in `NetServer::process_control_messages`. `NetServer::reject` carries the close. The client's closing-cause match in `main.rs` already falls unknown causes to `leave_session(LeaveCause::Refused)`, and it is also where end causes publish. `crates/net` gains a `blake3` dependency, which is already a workspace dependency.
- Two text fields on one screen: text-entry edits land on the active tree's `textEntryTarget` (`resolve_text_entry_intents`), but the dev keyboard reactions append to `ui.textEntry` explicitly. Settle how each field reaches its own slot on both input paths before building the menus.
- A connect started from the frontend skips the splash and first-launch hold that absorb a boot connect (`boot_sequence.md` §First-launch hold). A relevel reloading an installed backdrop as the host's level is specified, but off the boot path it is untested.
- Rival: netcode `Secure` mode, with the server key derived from the password, encrypts everything and saves a round trip. It loses the brief's terms: a wrong password reads as an unreachable host, so no `refusedPassword`; and a passwordless host cannot admit a client that holds one. Threat model, either way: anyone watching the LAN can capture a nonce and its proof and guess passwords offline. That is acceptable for an access gate.
- First slice: in-app join from the frontend against a CLI host on loopback, with no password, saves or UI. It falsifies the riskiest premise: that endpoint construction and mod-content seeding work after boot.
- `main.rs` is far past 800 lines. Put the new membership code beside `startup/session_membership.rs`, not in `main.rs`.

## Open questions
- How long a host waits for a password answer before refusing: keepalive alone, or a shorter bound — **delegated**.
- Remembered address field name and options-bridge mapping — **delegated**.

## Wire format
Mirrors the existing Control enums. Both enums are bitcode and append-only, so `PROTOCOL_ID` advances (`networking.md` §Version gates).

| Addition | Shape |
|---|---|
| `ServerControlMessage::PasswordChallenge` | `{ nonce: [u8; 32] }`, fresh per connection from the OS RNG, sent to a pending slot after its mod id matches |
| `ClientControlMessage::PasswordProof` | `{ proof: [u8; 32] }`, sent once per challenge |
| `ClosingCause::Password` | no fields, appended after `SessionEnded` |

`proof = blake3::keyed_hash(blake3::derive_key("postretro session password v1", password_utf8), nonce)`. An empty password derives a key like any other, so a client without one still answers. The host compares in constant time.

### Boundary inventory

| Name | Rust | Wire / serde | JS / TS | Luau |
|---|---|---|---|---|
| host verb | `SystemReactionKind::HostSession` | `"hostSession"` | `hostSession()` | `UI.hostSession()` |
| join verb | `SystemReactionKind::JoinSession` | `"joinSession"` | `joinSession()` | `UI.joinSession()` |
| slots | engine state catalog | `session.joinAddress`, `session.password`, `session.connection`, `session.endCause` | `getGameState().session.<name>` | same |
| end causes | client-side enum | `sessionEnded` `hostLost` `connectFailed` `refusedMod` `refusedVersion` `refusedPassword` `badAddress` `hostFailed` | same strings | same strings |
| mask | `WidgetDescriptor` text field | `"mask"` | `Text({ mask })` | `mask` |
| CLI | `parse_net_config` | `--password <text>` | n/a | n/a |
