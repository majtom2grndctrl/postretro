# Descriptor Identity & Naming Sugar — research notes

Verification log for the spec's source claims, plus derivation the spec body
does not need. Everything here was read this session.

## Verified claims

| Claim | Source | Status |
|---|---|---|
| Impact-event id requires ≥1 colon segment, `[A-Za-z0-9_.-]` per segment, ≤128 bytes | `sdk/lib/data_script.ts:435-440` (`validateImpactEventId`; diagnostic const at `:433`) | Confirmed. Luau twin at `sdk/lib/data_script.luau:751-774` implements the same rule imperatively. |
| Impact events register only through a manifest's `events`, by reference | `sdk/lib/data_script.ts:442` doc comment | Confirmed — but the id is **not** only a diagnostic handle; see contradiction 2 below. |
| Bind-failure diagnostic names the id | `crates/postretro/src/impact_policy.rs:190` `"[Impact] policy `{}` was skipped during bind"` | Confirmed. |
| `defineStore` builds refs as `Object.freeze({ slot })`, returns `Object.freeze({ declaration, state })` | `sdk/lib/data_script.ts:774`, `:776-779` | Confirmed exactly. Namespace is the first positional arg (`:759-762`); Luau twin `data_script.luau:984`. |
| Slots keyed by stable dotted names; table never cleared | `crates/entities/src/slot_table.rs:181-189` (struct doc), `context/lib/scripting.md:110` | Confirmed. |
| Local-only slot receives no `StateSlotId`, excluded from fingerprint | `crates/entities/src/slot_table.rs:43-60` (`ReplicationScope`, `None` variant doc at `:53`) | Confirmed. |
| Replicated schema entries sorted by dotted name, dense `StateSlotId` from 0, 32-byte fingerprint is the cross-peer gate | `crates/postretro/src/netcode/state_slots.rs:89-90`, `:113`, `:123`, `:305-307` | Confirmed. Schema maps `StateSlotId` back to dotted name for the apply path (`:64-68`). |
| `u16` wire discriminant numeric-equal to engine `ComponentKind`, drift-guarded both sides | `context/lib/networking.md:48-50`; constants `crates/net/src/wire.rs:101-110`; engine side `crates/entities/src/registry.rs:202` | Confirmed. The transferable shape: both sides independently assert one mapping; a pinning test fails on divergence. |
| Declaration attempts validate as a whole; changed schemas / duplicates / overlap reject the staged result | `context/lib/scripting.md:127`; `crates/entities/src/slot_table.rs:255-299` (`plan_reconcile`) | Confirmed. Overlap = equality or dotted-prefix (`slot_table.rs:431-439`). |
| `@`-prefixed names reserved, store declarations reject them | `context/lib/scripting.md:114`; `slot_table.rs:412-416` | Confirmed. Note: engine namespace validation checks only empty and `@` — no charset rule, so `:` is currently legal in a namespace (`slot_table.rs:405-429`). |
| `globalThis.<name>` rewrite is a bespoke post-bundle AST pass | `context/lib/scripting.md:269`; `crates/script-compiler/src/lib.rs:88-93`, `:134-142` (`ExportToGlobal` ordering), `:476+` | Confirmed as a pattern to copy. **Correction:** `bundle_with_dependencies` does *not* visit each parsed module — it builds a `Bundler` (`:105-117`), pops one merged module (`:122-130`), and runs every `visit_mut_with` on that merge. The only per-source-file site is `TsLoader::load` (`:199-288`: parse `:256`, resolver + strip `:275-276`, return `:283-287`), which is where a pre-bundle pass must hook. |
| `scripts-build` bundles entry with relative imports; Luau passes through unchanged | `context/lib/scripting.md:253`, `:259` | Confirmed. |
| Pre-stable: breaking changes allowed when call sites and tests move together | `context/lib/index.md:6` | Confirmed. |
| TS and Luau descriptor parsers are behavioral twins (validation/degradation) | `context/lib/scripting.md:17` | Confirmed — and `scripting.md:243-245` pins that twinhood is "module IDs and export vocabulary, not syntax", the precedent the TS-only compile sugar stands on. |
| Persistence: mod slots with `persist: true` save on clean exit, overlay once per process | `context/lib/scripting.md:129`; `crates/postretro/src/scripting/state_persistence.rs` (`STATE_FILE_PATH = "state.json"` `:16`, `CURRENT_STATE_VERSION = 1` `:15`, version gate `:109-114`, `is_persisted_mod_slot` `:171-175`) | Confirmed — **contradicts** the "nothing is on disk yet" framing; see below. |

## Contradictions with the task framing (carried into the spec)

1. **Something is already on disk.** `state.json` exists today: mod slots with
   `persist: true` save on clean exit keyed by authored dotted name
   (`state_persistence.rs:68-99`). The ordering argument survives — the data is
   dev-only and the version gate (`:109-114`) already ignores mismatched
   versions, so re-keying costs one version bump — but the spec must migrate
   this file now, not defer it to `E16--per-player-persistence`.
2. **The impact-event id is wiring, not only a diagnostic handle.**
   Base/override pairing keys by id (`impact_policy.rs:178-186`:
   `base_filters` lookup; unknown-override warn), and mod-global vs level-local
   composition replaces "for one id" (`scripting.md:43`). In-memory only —
   nothing persists or replicates by event id — so no durable key is needed,
   but qualification must preserve pairing.
3. **The mod folder is not the engine's mod identity.** `ModManifest.id` is
   required, gates multiplayer admission, and "the first committed id and
   version remain active across staged reloads" (`scripting.md:51`;
   `defineMod` doc `data_script.ts:677-690`). The folder is the boot-time
   selection handle (`--mod <path>`, `boot_sequence.md:19`), install-local, and
   mod discovery under `content/mods/` is explicitly not-yet-designed
   (`boot_sequence.md:206`). Concretely: the dev mod lives in `content/dev/`
   but its id is `postretro.dev` (`content/dev/start-script.ts:26`). Keying
   save data by folder would orphan it on reinstall into a different folder —
   the exact failure this spec exists to prevent. The spec therefore roots the
   namespace at `ModManifest.id`, which keeps every property the folder was
   chosen for (mod-level, unique, stable under internal reorganization, not
   the filename).

## Existing declarations to migrate

- `content/dev/scripts/coop-two-button-puzzles.ts:23` — `defineStore("coopPuzzles", …)`
- `content/dev/scripts/typed-handles-fixture.ts:52` — `defineStore("fixtureOpts", …)`
- `content/dev/scripts/combat-lifecycle.ts:18,56,81` — `defineImpactEvent("dev:…", …)` ×3 plus one `.override` (`:97`, needs no name — the handle carries the base id, `data_script.ts:400-408`)
- Engine/SDK tests and fixtures that hand-build declarations (e.g. `crates/scripting-core/src/store_bridge.rs`, `staged_manifest.rs` fixtures) follow the wire shape, which is unchanged.

## Lifecycle (durable-key mint → save → restore)

```mermaid
sequenceDiagram
    participant B as identity mint bin
    participant L as identity ledger (mod root)
    participant S as start-script (VM)
    participant C as mod-init commit
    participant T as SlotTable (memory)
    participant P as state.json (per-mod data dir)

    B->>S: run the real mod-init (QuickJS or Luau)
    S-->>B: ModManifestResult.store_declarations
    B->>L: append keys for new durable slots<br/>(append-only; temp-then-rename; write failure exits non-zero)
    S->>C: ModManifest.stores (authored names)
    C->>L: read + validate (engine never writes)
    alt persist/replicated slot has no entry (any build type)
        C-->>S: reject whole staged result,<br/>print paste-able entry
    end
    C->>T: commit slots keyed by authored dotted name
    Note over C,P: once per process, after first successful commit
    P->>C: v2 doc keyed by bare durableKey
    C->>T: overlay via ledger (durableKey → authored name)
    Note over T,P: clean exit: collect persisted mod slots,<br/>write v2 keys via ledger
```

Read call sites for every arrow: staged commit (`scripting-core/src/staged_manifest.rs` drain), overlay/save (`state_persistence.rs:68-99`, `:105+`; gating `StateStoreLifecycle` `:20-36`), replication schema build (`netcode/state_slots.rs:89-126`).

## Owner rulings — verification

Rulings folded into the spec; every relayed citation re-verified here rather
than taken on trust.

1. **Folder → `ModManifest.id` reversal ratified.** Spec keeps the id-rooted
   namespace; filename- and folder-derivation both stay recorded as rejected
   with reasons.
2. **Minting moved to the author's toolchain; the engine only reads the
   ledger, every build type.** Load-bearing argument verified:
   `crates/postretro/src/netcode/state_slots.rs:78-79` — "Both peers build
   this identically from their own slot tables; a fingerprint match is the
   cross-peer agreement gate" — so per-install minting lets the same mod
   diverge against itself; and `context/lib/networking.md:81-83` — "a parity
   mismatch **never closes the connection**… Content divergence is a
   diagnostic to a still-connected peer, not a disconnect" — so the failure
   is quiet, which is worse.
3. **The mint executes mod-init; it does not scan source.** Verified:
   - `postretro-script-compiler` cannot link `postretro-scripting-core` —
     the edge runs the other way, script-compiler is a build- and
     dev-dependency of scripting-core (`crates/scripting-core/Cargo.toml:24-29`).
     `crates/script-compiler/src/light_membership.rs:30-32` says so in
     comment form and hand-embeds a stub Luau SDK as a consequence. A
     declaration reader inside `scripts-build` would be a second
     implementation of `drain_store_declarations_js` /
     `drain_store_declarations_lua` (`store_bridge.rs:225`, `:256`).
   - `ScriptRuntime::run_mod_init` (`runtime/mod_init.rs:45`) dispatches by
     start-script extension to `run_mod_init_quickjs`
     (`runtime/mod_init_exec.rs:25`) or `run_mod_init_luau` (`:374`), then
     stores the manifest; `ModManifestResult.store_declarations`
     (`runtime/types.rs:116`) carries the validated declaration set. One
     mechanism, both runtimes.
   - Precedent for a non-engine `[[bin]]` in the engine package:
     `gen-script-types` (`crates/postretro/Cargo.toml:144-146`;
     `crates/postretro/src/bin/gen_script_types.rs`) builds `ScriptCtx`,
     `PrimitiveRegistry`, and `register_all` with no renderer and no session.
   - `xtask` links no engine crate; every subcommand spawns `cargo run`
     (`crates/xtask/src/main.rs:169-197`, `:227-259`, `:330-357`).
   - **Consequence found while working this through:** `run_mod_init` is the
     same call the Task 1 identity gate sits inside, so the mint would be
     rejected by the very ledger gap it exists to fill. Enforcement therefore
     becomes a `ScriptRuntimeConfig` flag (`runtime/types.rs:347-351`), off
     only for the mint. Pinned in the spec's Design and Task 1.
   - **`--mod-root` correction:** the flag already exists on `scripts-build`
     (`crates/script-compiler/src/main.rs:228-232`, gated to
     `--light-table`/`--manifest-out` at `:277-278`) and `prl-build` already
     passes it for map data-script compiles
     (`crates/level-compiler/src/main.rs:1185-1186`). The earlier design
     would have collided with that; the mint bin does not touch it, and the
     spec no longer claims `scripts-build` gains anything.
4. **`state.json` moves out of the working directory.** Verified: the path is
   `Path::new(STATE_FILE_PATH)` with `STATE_FILE_PATH = "state.json"`
   (`state_persistence.rs:16`), resolved against the process cwd at
   `crates/postretro/src/main.rs:3837` and
   `crates/postretro/src/startup/splash_lifecycle.rs:328`. Save rebuilds the
   document from the slot table alone (`:68-90`) and `fs::write` replaces it
   wholesale (`:166-168`), so a second mod launched from the same directory
   loses its rows at the first clean exit. The relocation target follows the
   options convention — `ProjectDirs::from("", "", "postretro")` then
   `config_dir().join(SETTINGS_FILENAME)` (`crates/postretro/src/options/mod.rs:228`;
   `directories` is already a workspace dependency, `Cargo.toml:137`,
   `crates/postretro/Cargo.toml:45`) — with save data under the data
   directory rather than the config one, plus a mod-id subfolder.
   `player_options.md:28-29` documents the atomic-write and
   corruption-fallback patterns to copy. Recorded in the spec as an intended
   behavior change: old working-directory saves are abandoned, not migrated,
   and the v1 → v2 bump is the natural moment.

**Superseded remedy.** An earlier revision had save *carry unmatched durable
rows forward*, to protect a co-tenant mod's rows in the shared file. Per-mod
subfolders remove the shared file, so there is nothing to carry forward;
unmatched rows are now simply dropped at the next save, which is what the
discard gesture already meant. Orderings rows 5, 6, and 15 reflect that.

**Watcher self-trigger hazard, resolved precisely.** The hazard does not
disappear at the watch layer — the mod root is watched non-recursively
(`crates/scripting-core/src/watcher.rs:328-338`), so a ledger write does
emit an event. It disappears at the classification layer: reload
classification is membership in the tracked mod-init dependency set
(`crates/scripting-core/src/staged_manifest/transfer.rs:59-64`; dependencies
collected from the TS compile's dependency report,
`staged_manifest.rs:228-297`), and `identity.json` is never a script
dependency. Pinned as Orderings row 14.

**Coverage, stated honestly.** Execution-based minting has no static blind
spot: computed schemas, computed names, and Luau start-scripts all mint,
because the mint runs the same code the engine runs. What remains TS-only is
the *binding-name sugar*, a syntax asymmetry sanctioned by
`scripting.md:243-245`. The engine's paste-able missing-entry diagnostic
stays as the backstop for hand-edited ledgers and for authors who forget to
re-run the mint.

## Direction questions worked (draft-plan §5b)

1. **Cause:** one string is both the author's reference and the durable key,
   so a rename is a data migration and the compiler cannot see it. Observed
   at: `state.json` keys, replicated-schema fingerprint input, and every
   hand-typed `dev:` prefix.
2. **Level:** identity is minted at the author-build seam (scripts-build,
   where binding names and schema literals exist), read and enforced at the
   mod-init commit seam (engine), validated at the SDK seam — each where
   the respective information exists, and the engine never a content
   mutator. Not per-feature: doing this inside `E16--per-player-currency`
   would retrofit currency later.
3. **Forecloses:** the ledger file becomes mod content with a format
   contract; `state.json` v2 key format; single-segment authored impact ids.
   All pre-stable and cheap to revise until real player data persists.
4. **Prior commitments:** scripting.md:110/:127/:114 (store contract),
   :17/:243-245 (twins are vocabulary, not syntax), networking.md:48-50
   (drift-guard shape), index.md:6 (pre-stable), §11 closed-vocabulary FFI.
5. **One-way door:** none until `E16--per-player-persistence` ships; after it
   ships, durable keys in player save files are permanent. That asymmetry is
   the sequencing argument.
6. **Strongest alternative:** keep the authored string as identity and
   document "never rename a persisted namespace." Rejected in spec body.
