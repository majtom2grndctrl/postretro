# coop--connect-adopts-host-mod

Brief · compact · reads: `context/lib/networking.md` §Two-gate handshake, §Admission and content parity, §Mod identity, §Role model · `context/lib/boot_sequence.md` §1 · `context/lib/build_pipeline.md` §Distribution packaging · read at 7eab3e3

## Problem
The owner asked for a capability: `postretro --connect <addr>` should need neither a map nor a mod, and the client should land wherever the host is. Its users are players and modders with more than one mod installed. The map half already works. A client with no map boots to the frontend, and the host's `Relevel` on admission loads the host's map through `follow_relevel_catalog`. The mod half does not. With no `--mod`, the client runs its own default mod (`content/dev`, derived from `DEFAULT_MAP_PATH`) before its first poll. The host's mod id reaches the client only inside the terminal `ClosingCause::ModId` refusal, and nothing finds a local mod by id. `postretro-tool run` and the dist launcher always pass `--mod`, so the gap is a raw `--connect` and a player choosing among installed mods. When this is done, a `--connect` with no `--mod` runs the host's mod if one with that declared id is installed locally. A `--connect` with `--mod` behaves exactly as it does today.

## Decisions
- **An explicit `--mod` wins.** Adoption applies only when `--mod` is absent and `--connect` is present. Admission still gates on mod id (`networking.md` §Admission and content parity), and the adopted mod must still clear it.
- **Lookup is local, by declared id.** The client resolves the host's id against the installed mod roots by each manifest's declared id, never by directory name. Without a match, the client gets today's typed refusal, naming the id it lacks. Nothing is downloaded or installed.
- **Adopt the id, not the version.** Admission compares only the id and logs the version, so adoption does too. A version or content difference surfaces as today's content-parity hold.
- **The map needs no change.** Map-less `--connect` already follows the host. This brief only states it in `docs/` for players, where every command must run from a bundle.
- **Non-goals.**
  - Content download or mod distribution.
  - A server browser or discovery (`networking.md` §Role model: direct connect only).
  - Dismissing the frontend menu on a relevel. The frontend hub owns it (`plans/done/sh-streaming--reveal-gate-and-warm-horizon`).

## Acceptance
### Automated
- [ ] A `--connect` boot with no `--mod`, against a host running mod `B`, with mods `A` and `B` installed: the client runs `B`, clears admission and participates on the host's map.
- [ ] Same setup with only `A` installed: the client is refused with a cause naming `B`, and no mod is loaded twice.
- [ ] A `--connect` boot with `--mod A` against host `B` is refused at admission exactly as today. It adopts nothing.
- [ ] A mod whose directory name differs from its declared id is found by the id.
- [ ] Single-player and `--host` boots choose their mod exactly as today.
### Manual
- [ ] Two processes, two installed mods: a bare `postretro --connect 127.0.0.1:<port>` lands in the host's game.

## Path
- Seams: `content_root_from_map` and the `--mod` parsing in `startup/session.rs`, `install_network_mod_content` (`startup/lifecycle_net.rs`), `ScriptRuntime::committed_mod_identity`, `ClosingCause::ModId` (`crates/net/src/wire/control.rs`), and the client's admission send in `NetClient::queue_control_messages`.
- Mod init runs before the first poll and freezes the mod identity (`committed_mod_identity`), so either boot waits for the host's id or the process restarts. That choice is the open question below.
- First slice: the local id → mod-root lookup, tested on its own. It is needed by either shape.

## Open questions
- Which shape? Owner: owner — **blocks build**. Recommended (a).
  - **(a) The host announces its mod id before admission.** This is a new Control message and an app protocol id bump. A mod-less `--connect` boot holds mod init at the frontend's world-less state until the id arrives, then runs mod init on the resolved root. It is cleaner for players, but it changes boot order: mod init moves after the first poll for this one path.
  - **(b) Relaunch on refusal.** The client reads `ClosingCause::ModId { expected }`, resolves it locally, and re-executes itself with `--mod <root> --connect <addr>`. There is no wire change and no boot-order change, but the process restarts visibly and handshakes twice.
- Where are "installed mod roots" enumerated? Siblings of the default content root, or an install-root `mods/` tree? — **delegated**: follow `build_pipeline.md` §Distribution packaging and report it in the plan of record.
