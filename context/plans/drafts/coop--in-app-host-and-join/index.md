# coop--in-app-host-and-join

Brief · resumable · Epic 15 · reads: `context/lib/networking.md` §Two-gate handshake, §Admission and content parity, §Leaving and ending a session, §Role model · `context/lib/ui.md` §3, §4 · `context/lib/scripting.md` §5, §10.4 · `context/lib/player_options.md` §2, §4 · `context/lib/boot_sequence.md` §1 · `context/research/coop-session-lifecycle.md` · read at 7ffced62

## Problem
The owner wants players to host and join co-op from an on-screen menu. Basis: a requested capability. Today role comes only from `--host` / `--connect` at launch, and only single-player can follow it (`networking.md` §Role model). No session password exists. Nothing tells a player why a connect failed or a session ended: the leave brief lands a client at its frontend silently and left the notice to this brief. When this is done, a player at the frontend hosts, optionally with a password, or joins by typing an address and password with keyboard or gamepad. They see that they are connecting or waiting for the host, then play in the host's level. When they are refused, lose the host, see the session end or fail to connect, the menu says which. Saved single-player progress (world and character) made before an in-app join survives the join and the later leave. Hosting or joining again needs no relaunch.

## Decisions
- **Runtime role entry, from the frontend only.** A single-player machine becomes a host or a client after boot, as many times as it likes. This narrows `networking.md` §Role model ("Hosting or joining again needs a relaunch"). The frontend means the frontend menu is up, no played level is installed, and none is queued or in flight; a backdrop counts as the frontend. A pending played load is excluded because a client must install only levels its host names (`networking.md` §Slot lifecycle). Hosting from a running level is a non-goal: install-only host registrations would have to replay against a live level, and nothing does that today.
- **One construction path for boot and entry.** Boot `--host` / `--connect` and in-app entry build the endpoint, seed network mod content and dev lag, and derive role-bound state through one path, so the two cannot drift. This extends the leave brief's one-derivation rule (`done/coop--client-leave-session`) to the entering direction.
- **Refusals.** Entering is refused with a warning off the frontend and in any role but single-player, and a refused verb changes no state. A second press while one waits is refused.
- **Deferral.** No player press is silently dropped: the menu must always be able to say what is happening (E23). A press made while a left level's unload or an ended host's close is pending waits, publishing `connecting`, and runs once both finish. It captures everything when pressed (clears `endCause`, parses the address, takes address and password), so what runs is what the player saw. A later navigation or level load cancels it, publishing `offline`.
- **Session verbs survive a leave.** A `hostSession` or `joinSession` after `leaveSession` in one reaction is exempt from the leave brief's drop of steps queued after a leave (`done/coop--client-leave-session`), and defers like a press. It belongs to the next session, not the world being left.
- **A join cancels an in-flight backdrop load** (`boot_sequence.md` §4, Load state machine). The host's relevel would discard it, and loading it first only delays the join.
- **The host's frontend is the interim gathering place.** An in-app host stands at its frontend with a live endpoint, and clients wait there held until it starts a level. The leave brief allowed this only for command-line hosts, "until that brief lands" (`done/coop--client-leave-session`), against "the frontend is outside every session" (`networking.md` §Leaving and ending a session). This brief makes it the player-facing flow until the optional-lobby brief gives hosts a lobby scene. Undo is cheap: that brief changes where the host stands, not how entry works.
- **Each hosting is a new hosted run.** Entry mints a fresh session id and seat counter, and seat 0 keeps its carried state, as an ended host keeps it (`done/E15--seat-session-identity-roster`; `networking.md` §Session-state ledger). A failed mint, bind or transport build leaves the machine single-player with its ledger untouched, as boot degrades, and sets `hostFailed`. No local-only id ever publishes.
- **Join saves first.** Before connecting, a join saves the world and character to disk through the clean-exit save's entry point, so the character/world save split (roadmap, next) changes one place. The leave-restore snapshot (`session.persisted_state`) and the join seed rebase on what it wrote, so a later leave restores the world as it was at join, not at boot. If the write fails, the snapshot rebases anyway and the failure warns.
- **Seat-0 carry across a join keeps today's behavior.** A join drops the seat ledger and a leave builds a fresh one, so carried loadout from earlier single-player levels does not survive. Entry is at the frontend, outside any run, and where carried gear lives is the saves brief's gear-scope decision.
- **Mods own the menus; the engine owns entry, admission and the facts.** `hostSession()` and `joinSession()` are reaction primitives beside `leaveSession()` (`scripting.md` §10.4). The engine adds no `ui.*` action and no engine-drawn dialog.
- **Engine session slots.** Each is client-local and nonpersistent:

  | Slot | Writable | Meaning |
  |---|---|---|
  | `session.joinAddress` | yes | Address the next join dials. Seeded at boot from the remembered address. |
  | `session.password` | yes | Password for the next host or join. Never logged. Cleared when hosting starts, when a join is admitted, and on `refusedPassword`. Kept on `connectFailed`, `hostFailed` and `badAddress`, so a retry needs no retyping. |
  | `session.connection` | no | `offline`; `connecting` (dialing, or a press waiting); `waiting` (admitted, not participating: the host is at its frontend or between levels); `playing` (participating). A host reads `session.hosting`. |
  | `session.waitReason` | no | While `waiting`: `host` (at its frontend, loading or between levels), `modDiffers` (the player's mod content differs from the host's: update the mod), `levelDiffers` (the host's level is missing or differs: get the map). Empty otherwise. Append-only; maps from the wire `HoldingCause`. |
  | `session.endCause` | no | Why the last session or attempt ended (vocabulary below). Cleared when the next host or join starts. |

  The research leaned toward a password never entering a store slot (`coop-session-lifecycle.md` §6). This diverges: text entry writes only slots. The slot never replicates, persists or logs.
- **End-cause vocabulary.** One value per way a session or attempt ends: `sessionEnded`, `hostLost`, `connectFailed`, `refusedMod`, `refusedPassword`, `badAddress`, `hostFailed`. A client's own leave and a host's own `endSession` set none. Before admission, every ending but an explicit refusal (`refusedMod`, `refusedPassword`) reads `connectFailed`: the player never got in. A different engine build fails the transport protocol id and reads the same, as does a `Protocol` refusal. Boot sets causes as entry does: a failed `--host` sets `hostFailed`, a malformed `--connect` sets `badAddress`. Later causes (eviction) append. Values are data; the mod supplies the wording.
- **Address grammar.** IPv4 literal (IPv4 only, `networking.md` §Role model), port optional and defaulting to the host default, in the menu and in `--connect` alike; `--connect` widens to match. Hostnames are refused, since DNS is a second failure mode with its own notice. A join parses the address first: a malformed one writes no save, leaves `session.connection` at `offline`, keeps the password, and sets `badAddress`. The address is remembered in player options (`player_options.md` §2) once a join is admitted, never on a refusal or a failed connect. The password is never remembered.
- **Session password is an engine admission gate.** It is checked after the mod id and before admission, by challenge and response (Wire format). The password never crosses the wire. A wrong answer, or no answer within a host-side bound, is a terminal refusal with `refusedPassword`, and the menu retries by reconnecting. A host without a password sends no challenge and admits anyone, including a client that holds a password. Nothing a client declared before admission applies until it is admitted. The password is fixed for a hosted run, so it passes admission's immutability test (`networking.md` §Admission and content parity). This gates access only; identity authentication stays a non-goal (`coop-session-lobby.md` §6).
- **CLI parity.** `--password <text>` sets the password for `--host` and supplies it for `--connect`. Both flags stay developer paths.
- **UI additions.** `Text` gains `mask: true`, which draws one `*` per grapheme of its bound content (ASCII renders in every bundled typeface). A masked Text must never expose its content to a screen reader. Text has no accessible value until E23 builds the snapshot, so the rule lands in `ui.md` §4.2 at promotion and E23 inherits it. The engine on-screen keyboard (`core/ui/keyboard.json`) gains `.` and `:` keys.
- **First consumer.** The dev frontend (`content/dev/scripts/frontend-menu.ts`) gains Host and Join submenus. Host takes an optional password; Join takes an address and a password. Both show each connection phase, each wait reason, and a notice per end cause.
- **Docs.** `networking.md` §Two-gate's "encrypted netcode handshake" is false under `Unsecure`: gate 1 is a `protocol_id` comparison. Correct it, §Role model, and §Session-state ledger's "Admission is a compatibility gate, not a trust decision", which a password makes an access decision.
- **Non-goals.**
  - Hosting from a running level. Warrant above.
  - Optional lobby and navigation, the join predicate, lifecycle reactions, `evictPlayer`: the lobby briefs and Phase 3.75.
  - Separate character and world save files, level-boundary saves, carried-gear scope: the saves brief.
  - Session traffic stays unencrypted. The password gates access; it does not make the session private (Path, rival).
  - Naming a version mismatch to the player: its own roadmap item, because it moves the build check out of the transport id.
  - Discovery, relay and NAT traversal: direct connect only (`index.md` §4).
  - Retrying a password inside one connection: every refusal is terminal today, and reconnecting costs one round trip.
  - IME, and AccessKit on text entry: E23.
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
- [ ] A join started while a backdrop load is in flight cancels it, and the client ends participating in the host's level with client-role install state and model prediction (P3).
- [ ] Host, end, join elsewhere, leave, host again: all in one process, with no relaunch.
- [ ] Host, admit a joiner, end, host again with no session between: the session id differs, the next joiner gets the seat id a fresh session gives, and seat 0 keeps its carried state (P15).
- [ ] Entry is refused with a warning, and no state changes (role, endpoint, seat ledger, `session.password`, `session.endCause`, `session.connection`, and no save is written), in each case: a played level installed, a played level's load queued or in flight, already hosting, and already a client. Each refused case is paired with one that is permitted, a backdrop load among them (P4).
- [ ] A press made while a left level's unload or an ended host's close is pending publishes `connecting`, then hosts or joins once both finish. A second press meanwhile is refused (P5).
- [ ] A failed session-id mint at host entry leaves the seat ledger untouched, stays single-player and sets `hostFailed`.
- [ ] One reaction firing `hostSession()` then `joinSession()`, and one firing `joinSession()` twice: the first verb takes effect, the second is refused, and exactly one join save and one endpoint result (P1).
- [ ] On a client, `leaveSession()` then `joinSession()` in one reaction leaves, then joins once the left level unloads. `leaveSession()` then any other queued step still drops at debug (P2).
- [ ] A waiting press captures address and password at the press: editing either while it waits changes nothing, and a malformed address never shows `connecting`. A level load or navigation started while it waits cancels it and publishes `offline` (P16).
- [ ] Boot `--host` and `--connect` produce the same role-bound state as in-app entry. The same compared fields match.
- [ ] One function builds an endpoint from a role, and boot and in-app entry both call it. No other non-test call of `NetEndpoint::from_role` exists. Grep gate.

**Saves**
- [ ] A world slot and a character slot written in single-player after boot are on disk once a join starts, and before its first connect packet.
- [ ] Join, then leave: the world equals the world at join, not at boot. The join seed carries the character as it was at join.
- [ ] A failed join save still rebases the restore snapshot, and warns.
- [ ] Join, then leave: seat 0's carried loadout is what a fresh single-player ledger holds.

**Password**
- [ ] The right password admits.
- [ ] A client admitted by password, whose parity, join seed and reveal reached the host before its proof, participates in the host's level with its join-seeded character (P6).
- [ ] Two clients pending on one host receive different nonces, a proof replayed from one connection onto another refuses with `refusedPassword`, and a proof with no challenge outstanding (passwordless host, already admitted, second proof) changes no slot state (P7).
- [ ] A client on the wrong mod is refused with `refusedMod` and receives no challenge, whatever its password (P9).
- [ ] Writing `session.password` on a live host changes which proof admits only at the next hosting (P10).
- [ ] A wrong password and a proof made from an empty password each refuse with `refusedPassword`, and the client lands at its frontend.
- [ ] A client that stays connected and never answers the challenge is refused with `refusedPassword` within the bound (P8).
- [ ] A proof drained in the poll its bound elapses admits. A host that ends while a challenge is outstanding sends `sessionEnded`. A host level change while a challenge is outstanding admits the proving client into the new level (P22).
- [ ] On a password host, a reconnecting client whose proof lands on its hold's deadline poll reclaims its seat, and one whose hold expires before its proof lands gets a fresh seat (P21).
- [ ] A host with no password (an empty slot) sends no challenge and admits both a client with no password and a client with one.
- [ ] No sent datagram, save file, replicated or persisted slot, or captured log line contains the password, and `session.password` holds it only until a verb consumes it. The test uses a password no fixture contains, scans captured bytes for it, and asserts no uncaptured worker-thread records.
- [ ] A client refused for its password applies no parity, join seed or reveal to the host.
- [ ] `--host --password p` admits a client answering `p` and refuses one answering `q`. `--connect --password p` is admitted by a host holding `p`.
- [ ] Wire round-trip and protocol-id tests cover the new variants, and the protocol id has advanced.

**Facts**
- [ ] Each end cause publishes on its own path, including a refusal whose cause arrives in the same update as the disconnect (P11). `badAddress` is checked against a malformed address, an IPv6 address and a hostname, none of which opens a socket, while an address without a port dials the default, from the menu and from `--connect`. A `Protocol` refusal publishes `connectFailed`.
- [ ] A `badAddress` join writes no save, never leaves `offline` and keeps `session.password` (P13).
- [ ] A host's own `endSession` sets no end cause. A failed boot `--host` sets `hostFailed`, and a malformed `--connect` sets `badAddress` (P14).
- [ ] `session.connection` steps `offline` → `connecting` → `waiting` (host at its frontend) → `playing` (host starts a level) → `waiting` (host changes level) → `playing` → `offline` (leave). A refused attempt goes `connecting` → `offline` and never shows `waiting`. The test records every published value, not one sample per frame, and includes a hold and re-promotion in one update and an admission with a session-ended cause in one update (P23).
- [ ] A deferred host press steps `session.connection` `offline` → `connecting` → `offline` as `session.hosting` publishes, and a deferred host press whose build fails ends `offline` with `hostFailed`. An undeferred host press never leaves `offline` (P18).
- [ ] `session.endCause` clears when the next attempt starts, and a client's own leave sets none. A host killed, or a session ended, before a pending join is admitted publishes `connectFailed` (P17).
- [ ] `session.waitReason` reads `host` while the host is at its frontend and between levels, `modDiffers` for a mod-digest hold, `levelDiffers` for a level identity or digest hold and a host level the client lacks, and empty while `playing` or `offline` (P19).
- [ ] `session.password` clears when hosting starts, when a join is admitted, and on `refusedPassword`. It is kept on `connectFailed`, `hostFailed` and `badAddress` (P20).
- [ ] The remembered address seeds `session.joinAddress` after a restart. It updates only when a join is admitted, never on a refusal, `connectFailed` or `badAddress` (P12).

**UI**
- [ ] `mask: true` draws one `*` per grapheme and an empty string for empty content.
- [ ] The on-screen keyboard types `.` and `:`.
- [ ] The Scripting surface example builds in a `content/dev` script.
- [ ] A late joiner's catch-up ignores `hostSession` and `joinSession` steps without a warning, as it ignores `leaveSession`.
- [ ] `UI.hostSession()` and `UI.joinSession()` in a Luau script produce the same steps as the TypeScript calls.
- [ ] The reserved `ui.*` actions gain no host or join action. Grep gate.
- [ ] `networking.md` no longer calls gate 1 an encrypted handshake or admission "not a trust decision", and §Role model no longer says hosting or joining again needs a relaunch. Grep gate.
- [ ] The dev frontend shows a notice for every end-cause value. The test derives the list from the end-cause enum.

### Manual
- [ ] Two machines on a LAN: host from the dev menu with a password, then join from the other by gamepad on the on-screen keyboard, and again by hardware keyboard.
- [ ] Each notice shows on a real client: wrong password, host ends, host killed, unreachable address after the retry window, a wrong-mod host, and a host on a different engine build (reads as a failed connect).
- [ ] A client that joins a host still at its frontend shows that it waits for the host, then plays once the host starts a level.
- [ ] Play single-player, join in-app, leave: the single-player world shows its pre-join progress.

## Path
- The boot path's role build lives in `Session::build` (`netcode::parse_net_config`, `NetEndpoint::from_role`, `authority_seat_table_or_local_only`, `published_host_address`). Extract it once so entry calls it, then call `App::install_network_mod_content`, which today runs only at mod init and on staged commit.
- The exit save is inline in `main.rs` (`collect_persisted_state`, `collect_persisted_faction_sentiment`, `collect_per_owner_state`, `merge_per_owner_state`, `save_persisted_state`). Extracting it is the join save's seam. `restore_store_after_leave` and `join_seed_from_persisted_state` read `session.persisted_state`.
- Admission is decided in `NetServer::process_control_messages`. `NetServer::reject` carries the close. The client's closing-cause match in `main.rs` already falls unknown causes to `leave_session(LeaveCause::Refused)`, and it is also where end causes publish. `crates/net` gains a `blake3` dependency, which is already a workspace dependency.
- The boot-time ledger is replaced by a fresh mint at host entry; `SeatTable` keeps its id and counter private, so this needs a new ledger operation that keeps seat 0's carry (`apply_role` keeps any existing ledger via `get_or_insert_with`).
- `waiting` and `playing` derive from the client's holding notices and participation marker; the host sends no admitted message, so admission is first visible as the `Relevel` or `Holding` notice sent at admission (`process_control_messages`); the roster follows later from `finish_host_poll`.
- Two text fields on one screen: text-entry edits land on the active tree's `textEntryTarget` (`resolve_text_entry_intents`), but the dev keyboard reactions append to `ui.textEntry` explicitly. Settle how each field reaches its own slot on both input paths before building the menus.
- A connect started from the frontend skips the splash and first-launch hold that absorb a boot connect (`boot_sequence.md` §First-launch hold). A relevel reloading an installed backdrop as the host's level is specified, but off the boot path it is untested.
- Rival: netcode `Secure` mode, with the server key derived from the password, encrypts everything and saves a round trip. It loses the brief's terms: a wrong password reads as an unreachable host, so no `refusedPassword`; and a passwordless host cannot admit a client that holds one. Threat model, either way: anyone watching the LAN can capture a nonce and its proof and guess passwords offline. That is acceptable for an access gate.
- First slice: in-app join from the frontend against a CLI host on loopback, with no password, saves or UI. It falsifies the riskiest premise: that endpoint construction and mod-content seeding work after boot.
- `main.rs` is far past 800 lines. Put the new membership code beside `startup/session_membership.rs`, not in `main.rs`.

## Open questions
- The value of the host-side bound on an unanswered challenge — **delegated**.
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
| slots | engine state catalog | `session.joinAddress`, `session.password`, `session.connection`, `session.waitReason`, `session.endCause` | `getGameState().session.<name>` | same |
| end causes | client-side enum | `sessionEnded` `hostLost` `connectFailed` `refusedMod` `refusedPassword` `badAddress` `hostFailed` | same strings | same strings |
| mask | `WidgetDescriptor` text field | `"mask"` | `Text({ mask })` | `mask` |
| connection phase | client-side enum | `offline` `connecting` `waiting` `playing` | same strings | same strings |
| wait reason | client-side enum | `host` `modDiffers` `levelDiffers` | same strings | same strings |
| CLI | `parse_net_config` | `--password <text>`; `--connect <ip>[:port]` | n/a | n/a |
