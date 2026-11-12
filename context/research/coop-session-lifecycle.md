# Co-op Session Lifecycle — Today and Target

> **Read this when:** deciding brief boundaries for leaving, kicking, host-to-menu, in-app hosting or joining, and run saves. It maps a session's whole life, and where save data is read and written along it.
> **Status:** research. The "Today" sections are read from source at 3fba30ae. The "Target" sections are a proposal: each item is marked **decided**, **leaning** or **open**, and nothing here binds until a brief or `context/lib/` records it.
> **Related:** [Networking](../lib/networking.md) §Role model, §Slot lifecycle, §Session-state ledger · [Scripting](../lib/scripting.md) §5 (Durable State Store), §10.4 · [Player options](../lib/player_options.md) · [Co-op session and lobby](coop-session-lobby.md) · `plans/drafts/coop--client-leave-session` · `plans/done/E16--per-player-persistence`

---

## 1. Today: host

Role comes only from CLI flags. A host's frontend is already lobby-like: clients connect and are admitted, and they wait as held slots (`HostLevelAbsent`) until a level installs.

```mermaid
stateDiagram-v2
    direction TB
    [*] --> HFrontend: launch with --host (CLI only)
    state "Frontend menu + backdrop<br/>clients admitted, held (HostLevelAbsent)" as HFrontend
    state "Loading → Settling" as HLoad
    state "Running<br/>revealed clients promoted, pawns spawned" as HRun
    HFrontend --> HLoad: loadLevel (menu button)
    HLoad --> HRun: install publishes parity, Relevel(life), spawn hints, then reveal
    HRun --> HLoad: loadLevel / restartLevel<br/>carry harvested, clients demoted, Relevel sent
    HRun --> HFrontend: returnToFrontend / ui.quitToMenu<br/>clients demoted and never told to unload
    HRun --> [*]: exit to desktop<br/>no session-end message, clients time out
    HFrontend --> [*]: exit to desktop
    class HRun ok
    classDef gap fill:#fde2e2,stroke:#b42318,color:#000
    classDef ok fill:#e3f4e8,stroke:#1a7f37,color:#000
```

A client disconnect holds its seat for 30 s (`HOLD_WINDOW`), whether the client disconnected cleanly or timed out. Reclaim needs a fresh `--connect` with the same player id.

## 2. Today: client

Shaded states are dead ends: the player has to relaunch, or the state is not what anyone intended.

```mermaid
stateDiagram-v2
    direction TB
    [*] --> CFrontend: launch with --connect ip:port (CLI only)
    state "Own frontend + own backdrop<br/>connecting, then admitted and held" as CFrontend
    state "Loading → Settling (host-named)" as CLoad
    state "Participating<br/>pawn spawned, snapshots flow" as CPart
    state "Held between host levels" as CHeld
    state "Local menu, still connected<br/>own New game refused" as CLocalMenu
    state "Stale level, no pawn, no menu" as CStale
    state "Disconnected client<br/>level verbs refused, no notice" as CDead
    CFrontend --> CLoad: Relevel → host-named load
    CLoad --> CPart: reveal → host promotes
    CPart --> CHeld: host changes level
    CHeld --> CLoad: Relevel (new life)
    CPart --> CLocalMenu: own quit, or mirrored returnToFrontend
    CLocalMenu --> CLoad: host's next new life pulls it back
    CPart --> CStale: host's own quit (only a Holding arrives)
    CStale --> CLoad: host's next Relevel
    CFrontend --> CDead: refused at admission (mod id)
    CPart --> CDead: host lost or exits
    CPart --> [*]: exit (no disconnect sent)
    class CLocalMenu,CStale,CDead gap
    classDef gap fill:#fde2e2,stroke:#b42318,color:#000
```

A connected client's `loadLevel` and `restartLevel` are always refused, including after its connection has ended.

## 3. Today: save data

Only two files reach disk. Health, inventory and life state cross level changes in memory only. Neither a run's position (current map, carried loadout) nor world state is saved anywhere.

```mermaid
flowchart LR
    S["settings.toml<br/>options + player_id"]
    J["#lt;data_dir#gt;/#lt;mod_id#gt;/state.json<br/>persist slots: globals + per-owner rows<br/>keyed by player_id, + faction sentiment"]
    Store["Slot table<br/>engine-global, survives levels"]
    Ledger["Seat ledger, memory only<br/>CarriedState per seat: health, ammo,<br/>weapons, life state, defense"]
    R1(["read before window"])
    R2(["read once, after first mod init"])
    W1(["host / SP: globals,<br/>clean exit only"])
    W2(["client: own per-owner rows,<br/>every 60 s + clean exit"])
    W3(["client: JoinSeed at admission"])
    W4(["harvest at unload,<br/>restore at install"])
    Host["Host store"]
    S --> R1 --> Store
    J --> R2 --> Store
    Store --> W1 --> J
    Store --> W2 --> J
    Store --> W3 --> Host
    Ledger --> W4 --> Ledger
```

Facts by symbol: `state_path`, `load_persisted_state`, `overlay_persisted_state`, `should_save_persisted_state` (gated on `is_connected_client`), `save_connected_client_per_owner_state`, `PER_OWNER_SAVE_INTERVAL`, `apply_join_seed`, `SeatTable`, `CarriedState` (not serializable), `harvest_bound_pawns`.

## 4. Gaps and hazards found

| # | What | Evidence |
|---|---|---|
| G1 | A client's quit keeps the connection, and the host's next new life pulls it back into play | `return_to_frontend` has no role branch; `follow_relevel_catalog` |
| G2 | Host's own quit to menu strands clients in a stale, pawnless level | `client_drain_control` on `Holding` only demotes |
| G3 | A lost host, an admission refusal, or a host exit leaves the client disconnected and still a client | `NetClient::update` only logs; `WorldLessPoll::Failed` acted on for host only |
| G4 | Exit to desktop sends no disconnect; the host waits out the timeout | no `NetClient` disconnect method |
| G5 | **Save leak on leave.** Once a client drops its endpoint, it saves globals at exit, and its slot table still holds the host's replicated values, so the host's world state would land in the ex-client's own `state.json` | `should_save_persisted_state(can_save, is_connected_client)` |
| G6 | No in-app host, join, leave or kick, and no player-facing connection state | reserved `ui.*` actions; `session.hosting` is display only |
| G7 | Globals save only at clean exit; a crash loses a whole session's progress | `scripting.md` §5 |
| G8 | No run save: a session cannot resume its map or loadouts later | `CarriedState` has no serde; `player_options.md` §1 "Game saves: TBD" |
| G9 | Stale docs: `coop-session-lobby.md` §3 and `research/netcode/index.md` non-goals deny client saves, but per-player persistence shipped | `done/E16--per-player-persistence` |

## 5. Target: verbs say intent

`returnToFrontend` mixes two jobs: **navigating** to the frontend scene and **membership** in a session. The target separates them, so no origin has to guess which one an author meant.

| Verb | Job | Host / single-player | Connected client |
|---|---|---|---|
| `returnToFrontend` | navigation | unload to menu (host: to the lobby, clients follow) | follow the host's lead; never ends the session |
| leave (new) | membership | n/a, or host ends the session | close cleanly → single-player frontend |
| `ui.quitToMenu` | both | return to the frontend (the host's open question: lobby or end) | leave, then frontend |
| kick (new, host) | membership | close a seat's slot with a cause | lands at single-player frontend |
| end session (new, host) | membership | close every slot, return to frontend | lands at single-player frontend |

Leaning: kick and end session are host-executed closes, because the host owns admission (owner, this session). Open: verb names and SDK shape, which are the owner's to design.

## 6. Target: session lifecycle

```mermaid
stateDiagram-v2
    direction TB
    state "Frontend<br/>single-player, no endpoint" as F
    state "Single-player level" as SP
    state "Hosting" as Hosting {
        state "Lobby<br/>frontend + live endpoint" as Lobby
        state "In level" as HLevel
        Lobby --> HLevel: start level
        HLevel --> HLevel: level change
        HLevel --> Lobby: back to menu [new]
    }
    state "Connected" as Connected {
        state "Waiting at own menu" as CWait
        state "Participating" as CPlay
        CWait --> CPlay: host starts level
        CPlay --> CPlay: host level change
        CPlay --> CWait: host back to menu [new]
    }
    [*] --> F: boot
    F --> SP: play
    SP --> F: return to frontend
    F --> Hosting: host game [3.75]
    F --> Connected: join [new]
    Hosting --> F: end session [new]
    Connected --> F: leave or host-side ending
    class Lobby,CWait new
    classDef new fill:#e0ecff,stroke:#1f4fb4,color:#000
```

- **Transitions:** clients follow the host into a level by Relevel, and the ledger carries across level changes. "Back to menu" is a host-sent signal clients follow while staying connected (G2). "End session" notifies every client. "Leave or host-side ending" covers the player's own leave plus a lost host, a refusal, a kick and an ended session: all land at a single-player frontend through the leave brief. Hosting and joining happen in-app.
- **Decided:** direct connect only (no discovery, relay or matchmaking); the engine owns admission, seats and map authority; the mod owns lobby policy (`coop-session-lobby.md` §2).
- **Leaning:** a lobby is the frontend with a live endpoint, not a new app state; session phase is a mod-declared slot.
- **Open:** whether the host boots into a lobby by default or the mod opts in (`coop-session-lobby.md` §5). Also whether a host's quit-to-menu goes to the lobby or ends the session.

## 7. Target: save data across a session's life

```mermaid
sequenceDiagram
    autonumber
    participant CD as Client disk
    participant C as Client
    participant H as Host
    participant HD as Host disk
    Note over C,CD: Boot
    CD->>C: state.json → own globals + own per-owner rows
    HD->>H: state.json → host globals (+ run save, if Continue) [open]
    Note over C,H: Join
    C->>H: JoinSeed: own per-owner persist values (shipped)
    H-->>C: replicated globals + own per-owner values
    Note over C,H: Play
    C->>CD: own per-owner rows every 60 s (shipped)
    H->>HD: globals at level boundary (+ run save) [proposed]
    Note over C,H: Leave or any host-side ending
    C->>CD: own per-owner rows, while the seat is still known [leave brief]
    CD->>C: re-read own state.json: drop host's replicated values [leave brief, fixes G5]
    Note over C: Single-player frontend: its saves are its own again
    Note over H,HD: Host ends session or exits
    H->>HD: globals (+ run save) (exit save shipped)
```

- **Shipped:** per-player progression is portable across hosts. Each player's per-owner `persist` slots live on their own device and travel up in `JoinSeed`. A host's globals live on the host's device.
- **Leave brief owes (fixes G5):** before dropping the endpoint, save the client's own per-owner rows. Then restore the client's store from its own `state.json`, so the host's replicated values never reach the ex-client's saves or its single-player world.
- **Proposed, open (G7):** the host also saves globals at level boundaries and at session end, not only at process exit.
- **Open owner decision (G8): does a co-op run resume across sessions?**
  - *Progression only* (Vermintide-style; shipped today): players keep unlocks and stats, and each session starts from a map the host picks.
  - *Run save* (campaign-style): at each level boundary the host serializes the session-state ledger: current map, each seat's `CarriedState` keyed by player id, and globals. A frontend "Continue" hosts from it. Rejoining players rebind by player id through the existing reclaim path. One format could serve three consumers: a run save, the revisited-map record (`roadmap.md` Future), and a host-migration handoff (Epic 15), since each packages the same enumerable ledger. Leaning: one format, decided before any of the three ships.

## 8. Proposed brief boundaries

| Brief | Owns | Depends on |
|---|---|---|
| `coop--client-leave-session` (reshape) | leave verb; `ui.quitToMenu` / exit-to-desktop leave on a client; any host-side ending → single-player frontend; save-then-restore on leave (G1, G3, G4, G5) | none |
| Host returns to lobby (new) | host-sent "at frontend" signal; clients follow to their menu, connected; host end-session verb (G2) | leave (end-session lands clients through it) |
| Phase 3.75 lobby authoring surface | in-app host, session phase, join predicate, joined/left/phase reactions, kick verb and policy | host returns to lobby |
| In-app join (new, small) | direct-connect UI: address entry, connecting and refused states, notice on host-side ending | leave (for the role change back) |
| Run save (new; the open decision in §7) | ledger serialization, level-boundary host saves, Continue | the owner's progression-vs-run decision |
