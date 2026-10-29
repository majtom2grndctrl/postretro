# coop--connect-adopts-host-mod

Brief · compact · reads: `context/lib/networking.md` §Admission and content parity, §Role model · `context/lib/boot_sequence.md` §1, §9 · `context/lib/build_pipeline.md` §Payload layout, §Authoring launch · read at 9a7c2ed

**Sequence:** the adoption half waits on mod discovery (`boot_sequence.md` §9, Mod discovery / multiple mod roots). The diagnostic slice builds now.

## Problem
The owner asked for a capability: `postretro --connect <addr>` should need neither a map nor a mod, and the client should land wherever the host is. Its users are players and modders holding more than one mod. Basis: a requested capability, not an observed defect. Nobody hits the gap today: the dist launcher and `postretro-tool run` always pass `--mod`, a payload holds exactly one mod at `content/<mod>`, and the repo holds one mod (`content/dev`). The map half already works: a map-less `--connect` boots to the frontend and follows the host's `Relevel` through `follow_relevel_catalog`. The mod half does not. Without `--mod`, the client runs the default mod before its first poll, and a mod-id mismatch ends in a terminal refusal whose client log reads "incompatible host: mod id mismatch" with no next step. When done: a refused client is told the host's mod id and how to relaunch with a matching mod; `docs/` says a raw `--connect` needs one; and, once mod discovery exists, a `--connect` with no `--mod` relaunches itself into the host's mod when it is installed locally.

## Decisions
- **Ship now: an actionable refusal.** On a mod-id refusal the client's diagnostic names the host's declared id and version and says to relaunch with `--mod` naming the installed folder whose mod declares that id. It names a folder, not the id, because `--mod` names a folder (`boot_sequence.md` §1) and the two differ (`content/dev` declares `postretro.dev`). The host's rejection log is unchanged.
- **Ship now: one `docs/` line.** A raw engine `--connect` needs a `--mod` matching the host's; `postretro-tool run` and the payload launcher already pass one. The command shown runs from an SDK bundle alone, so no cargo or xtask.
- **Adoption is by relaunch, gated on mod discovery as a prerequisite.** On `ClosingCause::ModId { expected, .. }` the client resolves `expected` through the mod-discovery index to a `content/<name>` sibling and re-executes itself with `--mod <name>` plus its other launch flags. A declared id exists only after the start script runs (`defineMod` → `ScriptRuntime::committed_mod_identity`); no static id record exists. So id lookup is mod discovery's design, which `boot_sequence.md` §9 already plans, and this brief neither builds a piece of it nor decides where installed mods live or how an id is read without committing mod init.
- **Why relaunch, not a pre-admission announce.** Announcing the host's id before admission would defer mod init past the first poll. Mod init supplies the frontend declaration, theme and fonts, the first-launch accessibility hold and the OS-preference wait (`boot_sequence.md` §1), so that reorder is expensive to undo. Relaunch keeps boot order and the wire unchanged. It extends E15's "a dropped peer relaunches" (`plans/done/E15--session-lifecycle`, Out of scope) from the player relaunching by hand to the client relaunching itself, for this one refusal.
- **Adoption applies only to a defaulted mod.** It runs when neither `--mod` nor a map argument chose the mod. An explicit `--mod` wins and is refused at admission as today. The relaunched process carries `--mod`, so it never adopts again.
- **Admission semantics are unchanged.** It still gates on the declared id alone, with the version diagnostic-only (`networking.md` §Admission and content parity). An adopted mod whose content differs surfaces as today's content-parity hold.
- **Layer placement.** The diagnostic and the relaunch are client-side app composition, above the net crate. The net crate gains nothing: `ClosingCause::ModId` already carries the host's id.
- **Non-goals.**
  - Content download or mod distribution.
  - A server browser or discovery of hosts (`networking.md` §Role model: direct connect only).
  - A player-facing refusal screen. Nothing above the transport shows connection state today (`networking.md` §Role model, Client connect lifecycle), and the frontend hub owns that surface.
  - Adopting on a protocol mismatch. A build mismatch has no local remedy.

## Acceptance
### Automated
**Diagnostic slice (buildable now)**
- [ ] A client refused for a mod-id mismatch logs one error naming the host's declared id and version and telling the player to relaunch with `--mod` naming the folder that holds that mod.
- [ ] A protocol-mismatch refusal logs as today and carries no `--mod` hint.
- [ ] A client admitted on a matching mod logs no refusal.

**Adoption (blocked on mod discovery)**
- [ ] A `--connect` boot with no `--mod` and no map, against a host running mod `B`, with `A` and `B` installed: the client relaunches once with `--mod` naming `B`'s folder and its other launch flags intact, clears admission and participates on the host's map.
- [ ] Same setup with only `A` installed: the client does not relaunch, and logs the actionable refusal.
- [ ] A mod whose folder name differs from its declared id is found by the id.
- [ ] A `--connect` boot with `--mod A`, or with a map argument selecting `A`, against host `B` is refused at admission as today and does not relaunch.
- [ ] A relaunched client refused again (the host changed mod meanwhile) does not relaunch a second time.
- [ ] Single-player and `--host` boots choose their mod as today.
### Manual
- [ ] The `docs/` line's command, run from an SDK bundle against a host on a matching mod, joins.
- [ ] Adoption (blocked on mod discovery): two processes, two installed mods, a bare `postretro --connect 127.0.0.1:<port>` lands in the host's game after one visible restart.

## Path
- Diagnostic seam: the `DivergenceReason::Closing` arm of `client_drain_control` (`crates/postretro/src/main.rs`). `DivergenceReason`'s `Display` (`crates/net/src/wire/control.rs`) also feeds the host's rejection log, so the hint belongs in the client arm, not in `Display`.
- Docs candidates: `docs/distribution.md` §What recipients should expect, or the co-op block in `docs/diagnostics.md`.
- Adoption seams, once discovery lands: the `--mod` parsing and `content_root_from_map` in `crates/postretro/src/startup/session.rs`, and the same client arm. The re-exec carries the original argv plus `--mod`.
- First slice: the diagnostic and docs line, which need no discovery and stay correct after it.

## Open questions
- Mod discovery's enumeration: where installed mods live (siblings of `content/<mod>`, or an install-root tree) — owner: owner — **blocks build** for the adoption half only.
- Mod discovery's id reading: how a declared id is read without committing mod init (a static record emitted at build, or a dry run of the start script) — owner: owner — **blocks build** for the adoption half only.
- Exact diagnostic wording and which `docs/` page carries the line — **delegated**: the executor decides and reports it in the plan of record.
