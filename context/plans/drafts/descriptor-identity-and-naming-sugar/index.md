# Descriptor Identity and Naming Sugar

## Goal

Split the one string that currently does two jobs in store and impact-event
declarations — the author's reference and the durable identity — so a rename
stops being a silent data migration, and stop requiring authors to retype
their mod's namespace at every declaration. Lands before
`E16--per-player-currency` (so currency is authored once in final form) and
before `E16--per-player-persistence` (the point after which a bad key is
permanent in player save data).

## Scope

### In scope

- Mod-derived namespaces: the engine qualifies impact-event ids and the
  replicated-slot identity strings with `ModManifest.id`; no declaration
  retypes it. Store names themselves stay bare — their cross-mod story is
  the durable key.
- Durable identity separate from the authored name: a per-mod identity ledger
  mapping authored slot names to opaque keys, minted by the author's
  toolchain and only ever read by the engine; persistence and the
  replicated-slot schema key by the durable form.
- The `scripts-build` binding-derived-name pass (TypeScript only, per source
  file, pre-bundle).
- The identity mint tool: a separate `mint-identity` `[[bin]]` in the
  `postretro` package that runs the mod's real mod-init and reconciles the
  ledger from the slots the mod actually declared, plus the `xtask`
  subcommand that drives it.
- Both SDK surfaces (`sdk/lib/data_script.ts` / `.luau`): new signatures and
  the revised id validation, as behavioral twins.
- Migration of existing declarations in `content/`; the `state.json` format
  version bump; and moving `state.json` off the working directory into a
  per-mod-id folder under the platform data directory.

### Out of scope

- The persistence mechanism itself — per-player save shape, join seed, who
  writes the file (`E16--per-player-persistence`). This spec only fixes what
  the *existing* global-store persistence and the replicated-slot schema key
  by.
- Per-player currency (`E16--per-player-currency`).
- The expression-dialect convergence problem (engine-state vs `runtime.*` vs
  impact-policy fluent methods) — separate spec.
- Binding-derived names for `defineReaction` — it already has an
  omitted-name path (content-hashed auto-id, `data_script.ts:565-575`) with
  documented cross-runtime limits; folding it in widens blast radius for no
  consumer.
- Multi-mod loading. One mod root is active per boot
  (`boot_sequence.md:206`); qualification is designed so a second mod cannot
  collide durably, but no multi-mod loader is built here.
- Engine-owned catalog namespaces (`player.*`, `session.*`, …). Engine slots
  get no ledger entries; their schema and persistence behavior is unchanged
  (persistence already filters to mod-owned slots,
  `state_persistence.rs:171-175`).

## Prerequisites

Sequencing, not code: this spec must land **before**
`E16--per-player-persistence` — persistence is what makes a bad key
permanent, and retrofitting durable identity onto already-saved player data
is the one genuinely expensive ordering mistake available. It lands before
`E16--per-player-currency` so per-player slots are declared through the final
authoring surface rather than migrated later. One caveat the framing missed:
global-store persistence already ships (`state.json`,
`state_persistence.rs:15-16`), so "nothing is on disk" is false today —
but that data is dev-only and the existing version gate
(`state_persistence.rs:109-114`) ignores old versions, so re-keying and
relocating it now costs one version bump and one abandoned file location.
That cheapness expires when player data ships.

A separate in-flight refactor on this branch moves pawn-to-seat ownership
onto `EntityRegistry` (netcode + entity registry files). Disjoint from this
spec's surface (`scripts-build`, SDK, store declaration/commit, persistence,
replicated-slot schema builder). Do not touch registry or netcode seat files
beyond the schema-builder keying in Task 6.

## Direction

**Problem.** A store namespace or impact-event id is one string doing two
incompatible jobs: the author's refactorable reference and the durable key
that persisted values (`state.json`, keyed by dotted name,
`state_persistence.rs:68-99`) and the replicated-slot schema (sorted and
fingerprinted by dotted name, `netcode/state_slots.rs:89-90`, `:123`) are
built from. Because they are the same string, a rename orphans data and the
compiler cannot catch it — the old name survives only in data. And because
identity is author-typed, every declaration retypes its namespace by hand
(`defineStore("coopPuzzles", …)`, `defineImpactEvent("dev:reward", …)` — the
colon segment is mandatory, `data_script.ts:435-440`).

**Prior commitments.**
- The store's contract keeps stable dotted names in unique namespaces with
  engine-global lifetime (`scripting.md:110`; `slot_table.rs:181-189`).
  Honored: in-memory addressing is untouched — dotted authored names remain
  the runtime key everywhere scripts, crossings, UI bindings, and IR leaves
  resolve. Durable keys appear only at the two seams that outlive a process
  or cross to a peer: the persistence file and the replicated-slot schema.
- Whole-attempt validation for staged declarations (`scripting.md:127`;
  `plan_reconcile`, `slot_table.rs:255-299`). Ledger validation joins that
  gate: a defect in identity rejects the whole staged result, exactly like a
  changed schema.
- `@`-reserved names (`scripting.md:114`; `slot_table.rs:412-416`) — kept.
- The wire-discriminant precedent (`networking.md:48-50`): a mapping two
  sides both depend on gets pinned once and drift-guarded on both sides. The
  authored-name ⇄ durable-key mapping is the same hazard; the ledger is the
  pin and Task 6's fingerprint tests are the guard.
- The half-split already in the engine: slots are keyed by dotted name
  (`slot_table.rs:181`) while a separate `StateSlotId` governs the replicated
  schema and local-only slots get none (`slot_table.rs:43-60`). That is not a
  complete answer: `StateSlotId` is dense, per-schema-build, and derived by
  sorting *names* (`netcode/state_slots.rs:89-90`) — it is positional, not
  durable, and persistence never sees it. It is, however, proof the engine
  already tolerates "runtime address ≠ schema key," which is the shape this
  spec completes.
- Behavioral twins are "module IDs and export vocabulary, not syntax"
  (`scripting.md:17`, `:243-245`). The compile-time sugar is TS-only syntax;
  the runtime validation it lowers into is twinned.
- Pre-stable: breaking changes move all call sites and tests together
  (`index.md:6`). The impact-id grammar change and `state.json` v2 rely on
  this.
- **Mod identity is `ModManifest.id`, and the namespace root derives from
  it.** The id is required, gates multiplayer admission, and the first
  committed id stays active across staged reloads (`scripting.md:51`); the
  folder is the install-local boot selection handle (`--mod <path>`,
  `boot_sequence.md:19`), and the dev mod already demonstrates the split —
  folder `content/dev/`, id `postretro.dev`
  (`content/dev/start-script.ts:26`). The id carries every property the
  namespace root needs: mod-level, unique per session by construction,
  stable under any internal file reorganization, and not the filename.
  Filename- and folder-derivation are rejected below.

**Alternatives rejected.**
- *Filename-derived namespaces* — rejected before this spec and recorded
  here: two mods can each ship `combat.ts`, and moving a file inside a mod
  would move the namespace. The wrong axis entirely.
- *Folder-derived namespaces* — the folder is chosen by the installer, not
  the author, so it is mutable exactly where identity must not be:
  reinstalling the same mod into a different folder would orphan its data,
  the defect this spec exists to remove.
- *Keep the authored string as identity, document "never rename."* The
  strongest rival: zero new mechanism. Rejected because it is unenforceable
  by construction — the compiler cannot see names that exist only in saved
  data, so the rule's first violation is silent data loss, discovered by a
  player. The entire premise of the drift-guard precedent is that
  load-bearing string agreement gets machine checking, not documentation.
- *Content-hashed identity* (key = hash of schema): a schema edit — adding a
  field, changing a default — would change the key and orphan data exactly
  like a rename does today. Identity must not derive from anything mutable;
  a hash of mutable content is mutable.
- *Authored-name-as-key plus explicit rename migrations* (the
  Factorio/datafixer shape): keep the dotted authored name as the durable
  key; a rename ships an `old → new` migration entry in a mod-content file,
  applied at persistence overlay. Cheaper mechanisms — no engine-written
  file, no dev/release split, human-readable save keys. Rejected because a
  forgotten migration is exactly as silent as today's forgotten rename: it
  surfaces only when stale data meets the overlay, i.e. in a player's old
  save. The ledger makes the mapping complete and machine-checked at every
  commit regardless of whether old data exists — the same property the
  wire-discriminant drift-guard (`networking.md:48-50`) and whole-attempt
  staged validation (`scripting.md:127`) already chose over
  detect-on-collision. It also degrades better under chained renames: edit
  one ledger line versus resolving a transitive migration chain.

## Design

**Authored names.** `defineStore(name, schema)` — the first argument becomes
the store's *authored name* (today's "namespace" argument, unchanged wire
field). `defineImpactEvent(name, filter, build)` — the authored id becomes a
**single segment**: 1–64 bytes of `[A-Za-z0-9_.-]`, no colon
(`validateImpactEventId` inverts its colon rule; the colon is now reserved
for engine qualification). Store authored names and slot names additionally
reject `:` engine-side, in `validate_namespace_records`
(`slot_table.rs:405-429` — today it checks only namespace emptiness, leading
`@`, and slot-name emptiness/collision; no charset rule exists on either
half) and in the descriptor parser where the slot-name `@` rule already
lives (`store_bridge.rs:374-380`). The `@` rules are unchanged.

**Mod qualification — impact events.** At composition the engine forms
qualified impact-event ids `<modId>:<authoredId>` — never the author. Stores
are not qualified by name anywhere: their cross-mod story is the durable key
(below), and their in-memory dotted names stay bare. `ModManifest.id` is
already validated by `validate_mod_manifest_id`
(`scripting-core/src/runtime/types.rs:383-385`) through the shared
`postretro_foundation::validate_ascii_identifier`
(`foundation/src/data_descriptors/validate/foundation.rs:29-51`), whose
charset **includes** `:`. The change is a colon *subtraction*, applied in
`validate_mod_manifest_id` only — not in the shared helper, which also
serves ammo-type and weapon-credit identifiers (`impact_policy.rs:436`,
`combat.rs:143`, `:204`, the js/lua reaction parsers) whose grammar keeps
`:`. Impact events use the qualified id for composition and diagnostics:
base-filter inheritance keys by it (`impact_policy.rs:178-186`), per-dispatch
same-id eviction compares it (`impact_policy.rs:219-234` — `BoundPolicy.id`
carries the qualified form, which is what AC 4's override eviction runs on),
and the bind-skip warning (`impact_policy.rs:190`) prints it. Impact events
get **no durable key**: no engine surface persists or replicates by event id
— the id's only consumers are in-memory composition and logs, both rebuilt
every install.

**Durable identity — the ledger.** A mod ships
`<mod_root>/identity.json`: `{ "version": 1, "slots": { "<authored dotted
slot name>": "<durable key>" } }` — the map is keyed by fully dotted *slot*
names, one entry per slot, not per store. A durable key exists for every
mod-declared slot that has a durable consumer — `(persist && !readonly)`,
matching the save filter `is_persisted_mod_slot`
(`state_persistence.rs:171-175`; a readonly persist slot is never saved), or
`network != None` (`slot_table.rs:43-60`) — and for no other slot (a
local-only ephemeral slot has no reader that outlives the process; no
consumer, no entry). Keys are `k` + 16 lowercase hex chars from the OS RNG,
opaque, globally unique by construction, never derived from the name or
schema. The persisted key is the **bare durable key** — no mod-id prefix.
`committed_mod_identity` is seeded once per process
(`runtime/mod_init.rs:164-170`; `None` at construction,
`runtime/core.rs:146`), so a mod id is frozen only *within* a run: an author
editing it between runs would silently orphan every prefixed row, making the
on-disk form derive from something mutable — exactly what the key exists to
avoid. The prefix would also buy nothing 64 bits of OS randomness does not
already provide. The replicated-slot schema is different: it is rebuilt per
process on both peers from identical content, so its identity string may
carry the id (Task 6).

**The author's toolchain mints; the engine only reads.** Minting is an
explicit author invocation, not a side effect of compiling. A `mint-identity`
binary — a third `[[bin]]` in the `postretro` package, beside the engine and
`gen-script-types` (`crates/postretro/Cargo.toml:140-146`) — constructs the
real runtime, calls `ScriptRuntime::run_mod_init(mod_root)`
(`runtime/mod_init.rs:45`), reads back `ModManifestResult.store_declarations`
(`runtime/types.rs:116`), and reconciles `<mod_root>/identity.json` from the
slots the mod actually declared: append an entry for each durable slot
missing one, never modify or delete an existing line, exit non-zero if the
file cannot be written. `cargo run -p xtask -- mint-identity <mod-root>`
shells out to it: xtask never links engine crates, every subcommand spawns a
`cargo run` (`xtask/src/main.rs:169-197`, `:227-259`, `:330-357`), and this
follows that shape. The engine, dev and release alike, reads the ledger at
mod-init commit and rejects the whole staged result on a missing entry, with a
diagnostic naming the slot and printing a ready-to-paste entry carrying a
freshly generated key. The ledger is author-owned JSON; hand-editing it is a
first-class path, not a fallback.

**Why execution, not a static scan.** Reading `defineStore` calls out of
source would be a second implementation of declaration parsing, and it could
not live where it would need to. `postretro-script-compiler` structurally
cannot link `postretro-scripting-core`: the edge runs the other way
(`scripting-core/Cargo.toml:24-29` — script-compiler is a build- and
dev-dependency only), which is why the compiler's light-membership evaluator
had to hand-embed a stub Luau SDK instead of reusing the runtime's
(`script-compiler/src/light_membership.rs:30-32` states it directly). Any
declaration reader inside `scripts-build` inherits that trap and drifts from
the real validators (`drain_store_declarations_js` /
`drain_store_declarations_lua`, `store_bridge.rs:225`, `:256`). Executing
buys three things at once. `run_mod_init` already dispatches by extension to
`run_mod_init_quickjs` (`mod_init_exec.rs:25`) and `run_mod_init_luau`
(`:374`), so **one mechanism covers both runtimes** — Luau is not left to
hand-editing. The mint sees computed schemas and computed names, which no
static scan can. And it mints from exactly the code path the shipping engine
runs at commit, so a slot that mints is a slot that commits. Precedent for
the shape: `gen-script-types` (`crates/postretro/src/bin/gen_script_types.rs`)
is already a `[[bin]]` in the engine package that stands up `ScriptCtx` and
the primitive registry with no renderer and no live session.

**The mint bin is not the engine.** The rule this spec keeps is that the
engine never writes into a mod root, and it holds without exception: the
`postretro` binary writes nothing there, in any build type. A
separately-named authoring tool that no player invokes sits on the author's
side of that line, where `scripts-build` and `prl-build` already sit. Sharing
a Cargo package with the engine does not move it across. Stated explicitly so
the carve-out is not read as a violation later.

**The identity gate must be switchable for the mint.** Task 1 places the
ledger check inside the same commit path the mint runs, so a mint against an
incomplete ledger would reject before it could append. Identity enforcement
is therefore a `ScriptRuntimeConfig` policy (`runtime/types.rs:347-351`):
the engine constructs with it on, the mint bin is the one caller that
constructs with it off, and nothing else ever does.

**Ledger writes are atomic.** One deliberate invocation writes at a time, so
there is no concurrency to arbitrate — but a truncated ledger is still the
worst artifact available, since it is the file the engine refuses to boot
without. Serialize to a sibling `.tmp` and rename over the target, the
pattern `player_options.md:28` already documents for settings; not the plain
truncate-and-write at `state_persistence.rs:166-168`.

Why the mint cannot be per-install: both peers build the replicated-slot
schema from their own slot tables and "a fingerprint match is the
cross-peer agreement gate" (`netcode/state_slots.rs:78-79`). Two installs
of the same mod holding independently minted keys compute different
fingerprints, and **no gate catches that**: content parity compares only
the mod digest — trigger events, trigger pools, and crossings
(`mod_digest.rs:20-33`) — and the level identity/digest
(`net/transport.rs:126-127`, `:485-497`); the schema fingerprint is in
neither domain. What actually happens: the host stamps its fingerprint into
every snapshot, the client's batch validation returns
`SchemaFingerprintMismatch` (`net/state_slots.rs:505-506`), and the whole
state batch is dropped while existing values are kept
(`netcode/state_slots.rs:802-808`, warn at `:957-963`). The peer stays
**participating**; replicated store state — including engine `player.*`
slots, which share the one schema — silently never applies, every frame,
for as long as the session lasts. Quieter than a crash, and therefore
worse. So a key is minted once, by the author, and ships with the mod.
Author-side minting also gives dev and release one read-only runtime path,
so no dev/release divergence class exists here. And it makes identity
independent of whether mods eventually ship compiled output or raw sources
to players: that ship-format question stays open elsewhere and never
becomes an input to identity. The price is one command the author must
remember to run; the engine's paste-able missing-entry diagnostic is what
makes forgetting it loud rather than silent.

**Rename and discard.** A rename is: change the name in the script *and*
edit the ledger entry's authored-name side, keeping the key. Deleting a
ledger entry is the explicit "discard that data" gesture — the next mint
issues a fresh key and old durable rows go unmatched. An accidental rename
(script changed, ledger not) is loud, not silent: the next mint appends a
new key for the new name *and* the commit warns that the old entry matches
no declared slot, so the recovery (merge the two lines) is visible before
any release.

**Where durable keys are used.** Exactly two seams:
1. *Persistence.* `state.json` becomes version 2; slot keys are the bare
   durable keys. Save resolves authored → key through the ledger; the
   one-time overlay (`state_persistence.rs:105+`) resolves key → authored.
   A v1 document is ignored by the existing version gate with a warning —
   accepted loss, dev-only data, pre-stable. **The file also moves out of
   the working directory**, to `<platform data dir>/postretro/<modId>/state.json`.
   Today the path is a bare relative string (`STATE_FILE_PATH =
   "state.json"`, `state_persistence.rs:16`, resolved against the process
   cwd at `main.rs:3837` and `splash_lifecycle.rs:328`), so two mods
   launched from one directory share one file — and since save rebuilds the
   document from the slot table alone (`:68-90`) and replaces the file
   wholesale (`:166-168`), the first clean exit destroys the other mod's
   rows. Resolution follows the convention player options already ship
   (`ProjectDirs::from("", "", "postretro")`, `options/mod.rs:228`;
   `player_options.md:24`), with the mod-id subfolder doing what the durable
   key does inside the file: separating one mod's data from another's. The
   atomic-write and corruption-fallback patterns come from the same place
   (`player_options.md:28-29`).
2. *Replicated-slot schema.* Wherever the schema builder uses a mod slot's
   dotted name today — sort order for dense `StateSlotId` assignment, the
   fingerprint stream, the id↔name apply mapping
   (`netcode/state_slots.rs:62-74`, `:89-126`) — it uses a **replication
   identity string** `<modId>:<durableKey>` for mod-declared slots. The
   prefix is safe here where it is not on disk: the schema is rebuilt per
   process on both peers from identical content, so the id can never drift
   between mint and read. Engine-catalog slots keep their dotted names
   (both peers run handshake-matched engine versions; the catalog needs no
   rename protection). Both peers hold the same mod content including the
   ledger — the mod id already gates admission — so fingerprints still
   agree by content.

**Relocating `state.json` is an intended behavior change.** Whatever sits at
an old working directory is abandoned, not migrated: nothing reads it again.
That is deliberate. The v1 → v2 key change this spec already makes is the one
moment the file is being invalidated anyway, so moving it costs a single
abandonment instead of two, and the data in question is dev-only today
(`state_persistence.rs:15-16`, pre-stable). Doing it after
`E16--per-player-persistence` would mean migrating real player saves twice.
One honest caveat: keying the subfolder by `ModManifest.id` means an author
who changes their id orphans the old folder. That is a strictly better
failure than today's — an orphaned directory still holds its bytes and can be
renamed back, where the shared-file overwrite it replaces destroys another
mod's rows with no recovery at all.

Everything else — `SlotTable` keys, `StateRef.slot` strings, crossing and UI
bindings, IR input leaves, HUD publisher — continues to read authored dotted
names, unchanged.

**Naming sugar.** `scripts-build` gains a second bespoke AST pass, modeled
on the `globalThis` rewrite (`scripting.md:269`;
`script-compiler/src/lib.rs:476+` — that pass runs prelude-only, gated at
`lib.rs:137-142`, so this is a sibling pattern, not a shared hook), running
as each source file is parsed, before bundling (bundling inlines relative
imports, `scripting.md:253`, and may rename top-level bindings, so the
authored name is gone after). It rewrites a
**direct assignment** whose initializer is a `defineStore` /
`defineImpactEvent` call missing its leading name string —
`const progression = defineStore({ … })` becomes
`defineStore("progression", { … })`. Only direct `const`/`let`/`var` (and
`export const`) assignments fire; a call inside a helper function, an array,
an object property, or a bare expression is left untouched — that is the
feature that stops a helper donating its own name to every descriptor it
builds. At runtime, both SDKs accept the name-less overload only to throw a
diagnostic stating the rule ("name required: the binding-name sugar applies
only to direct `const name = define…` assignments compiled by
scripts-build"), so a helper-built or Luau name-less call fails at script
load, not silently. The explicit form stays legal everywhere and is the only
form in Luau — sanctioned syntax asymmetry per `scripting.md:243-245`.

## Orderings

| # | Scenario / ordering | Expected outcome |
|---|---|---|
| 1 | Author adds a durable slot, then runs the mint | The mint runs the mod's real mod-init, sees the new slot in `store_declarations`, appends one entry. Re-running appends nothing — mint once. Nothing about compiling touches the ledger: neither the dev-startup auto-compile nor `prl-build`'s data-script compile has any part in minting. The mint bin is built with `debug_assertions`, so `run_mod_init` recompiles a stale `start-script.js` from `.ts` first (`mod_init.rs:87-100`) and mints against current sources; the xtask subcommand builds the `scripts-build` sidecar before invoking it, for that reason alone. |
| 2 | Ledger write fails during the mint (read-only mod root) | Mint exits non-zero naming the ledger path. The temp file is discarded and the existing ledger stays byte-identical, so the engine never sees a half-minted ledger. An unwritable key would be a key that changes on the next mint. |
| 3 | Durable slot in the attempt's declarations missing a ledger entry | Debug: whole staged result rejected. Release: fatal boot error, exit non-zero (see Task 1 — release has no staged commit to reject). Both diagnostics name the slot and print a ready-to-paste entry with a fresh key — the remedy when the mint has not been re-run, or a hand-edited ledger is short an entry. |
| 4 | Authored rename with ledger entry updated, across restart | v2 save row (durable key unchanged) overlays into the renamed slot; fingerprint unchanged. |
| 5 | Authored rename in script only (ledger untouched) | Next mint appends a new key for the new name; commit warns that the old entry matches no slot. Old save row goes unmatched and is dropped at the next clean-exit save. Recovery: merge the two ledger lines before that save, or restore the row by hand. |
| 6 | Ledger entry deleted, save row still present | Row is an unknown durable key at overlay: warn, ignore, slot starts at default. The next save rebuilds from the slot table and the row is gone — the discard gesture is complete once, deliberately. |
| 7 | Mid-session staged reload renames a committed namespace (ledger edited with it) | Reconciler sees delete+add by authored name: the new namespace — declared in the attempt, entry present — commits at defaults; the removed one keeps its values live (`scripting.md:127`: removed declarations do not clear committed stores) but has no ledger entry anymore, so save skips it with a warning. Durable identity protects data across process runs, not across an in-session rename; overlay never re-runs (once per process). After restart, only the renamed slot exists and the old rows overlay into it. |
| 8 | Staged reload changes the durable key of a committed slot | Reject whole staged result (identity change on live state), same class as `IncompatibleSchema`. |
| 9 | Two ledger entries share one durable key | Reject at ledger validation (staged result in debug; fatal boot error in release). |
| 10 | Ledger entry names a slot that is neither persisted nor replicated | Warn (stale grant of durability), entry retained; not an error, because removing `persist` then restoring it must not re-mint. |
| 11 | Two same-name `defineImpactEvent`s in one mod | Unchanged from today: last-registered wins per composition rules (`scripting.md:43`) — qualification adds the same prefix to both, so it neither creates nor masks the collision. |
| 12 | Sugar target shadows an SDK global (`const world = defineStore({…})`) | Rewrite fires on the binding name like any other; name validity is the SDK validator's job, not the compiler's. |
| 13 | v1 `state.json` sitting in a working directory | Never read again — v2 lives under the platform data dir, keyed by mod id, and nothing consults the cwd. The abandonment is intended (see Design); the file is left in place rather than deleted, since the engine does not own that directory. A v1 document found at the *new* path is ignored with the existing version-gate warning (`state_persistence.rs:109-114`) and replaced at the next clean exit. |
| 14 | Mint appends to `identity.json` while an engine with the hot-reload watcher is running | The mod root is watched non-recursively (`watcher.rs:328-338`), so the write emits an event — but reload classification is exact-path membership in the active mod-init dependency set (`changed_paths_affect_mod_init`, `runtime/types.rs:217-254`, membership check at `:242`), and the ledger is never a script dependency. No reload triggers. |
| 15 | Two branches each add the same new slot, then merge | Distinct keys minted per branch. A merge keeping both lines is a duplicate authored name — rejected at the *deserializer* level (Task 1; a plain map deserialization would collapse duplicates last-wins before any validator ran); a resolved merge keeps one key and the losing branch's dev `state.json` rows go unmatched (dev-only loss, warned at overlay). |
| 16 | Two peers co-op the same mod with ledgers that have drifted | Different fingerprints, and no admission or parity gate catches it (the mod digest covers trigger events/pools/crossings only, `mod_digest.rs:20-33`). Every snapshot's state batch is dropped client-side with the stable `[Net]` mismatch warning (`netcode/state_slots.rs:802-808`, `:957-963`) while the peer stays participating — replicated store state, engine `player.*` included, never applies. The fix is content, not netcode: commit one ledger. |
| 17 | Ledger entry deleted mid-session, then a staged reload declares the slot again | The attempt's declaration has no entry — staged result rejected until a fresh key exists (re-run the mint, or hand-add). Data under the old key is unreachable from then on: the discard outcome. |
| 18 | Ledger file unparseable, or `version != 1` | Reject (staged result / fatal boot error), naming the parse error or version. Never ignore-with-warning: the `state.json` degrade precedent (`scripting.md:129`) is wrong here, because loading with defaults while durable data exists reproduces the orphaned-data failure this spec removes. |
| 19 | Ledger file absent | Zero durable slots declared: normal load, no diagnostic. One or more durable slots: the missing-entry rejection (row 3). |
| 20 | Two mods launched from one working directory | Separate `state.json` files under separate mod-id subfolders. Neither clean exit can reach the other's rows — the failure the relocation removes. |
| 21 | Platform provides no data directory (`ProjectDirs` returns `None`) | Warn once; skip both the restore and the clean-exit save for that run. Never fall back to the working directory: a silent cwd fallback reinstates exactly the shared-file collision this move exists to end. |
| 22 | Author changes `ModManifest.id` between runs | The new id resolves to a new, empty subfolder; the old one is orphaned and its rows are unreachable, with the same unknown-slot silence as a fresh install. Recoverable by hand (rename the directory), unlike the overwrite it replaces. |

## Acceptance criteria

- [ ] Renaming a persisted store slot's authored name (script + ledger entry
  edited together, key preserved) across an engine restart restores the
  pre-rename value into the renamed slot; deleting the ledger entry instead
  leaves the slot at its default and warns naming the orphaned save row.
- [ ] A `defineStore` / `defineImpactEvent` written as a direct
  `const x = define…({…})` assignment with no name string commits under the
  binding's name. The same name-less call inside a helper function fails at
  script load with a diagnostic stating the direct-assignment rule — in both
  runtimes, same diagnostic text. The explicit-name form works at every call
  site in both runtimes.
- [ ] An authored impact-event id containing a colon is rejected with the
  same diagnostic in TS and Luau; engine logs (bind-skip, unknown-override)
  print the mod-qualified id.
- [ ] The combat-demo zombie override still evicts its base after
  qualification: base and override pair by qualified id with no authored
  prefix in either.
- [ ] The replicated-slot schema fingerprint is unchanged by an authored
  rename with preserved durable key, and changed by a durable-key change
  with preserved authored name (both directions asserted — the drift-guard
  pair).
- [ ] Running the mint against a mod that declares a new durable slot appends
  exactly one `identity.json` entry; re-running appends nothing; existing
  lines are never modified or deleted; an unwritable mod root exits non-zero
  and leaves the existing ledger byte-identical. A durable slot missing its
  entry rejects the whole staged result in debug and is a fatal boot error
  (non-zero exit) in release — both naming the slot and printing a
  ready-to-paste entry — and in both cases the `SlotTable` is left unmutated
  by the failed attempt.
- [ ] With a complete ledger and a read-only mod root, dev and release
  engine builds load the mod identically; the engine writes nothing into
  the mod root.
- [ ] A clean exit writes v2, keyed by bare durable keys, to
  `<platform data dir>/postretro/<modId>/state.json` — through a temp-file
  rename, never a truncating write — and writes nothing to the working
  directory. Two mod ids run from one directory keep independent files, and
  neither exit disturbs the other's rows. A v1 document at that path is
  ignored via the version gate with a warning. A duplicate durable key or
  duplicate authored name in the ledger rejects (staged in debug, fatal boot
  in release).
- [ ] Staged hot reload: an identical redeclare with an unchanged ledger
  preserves values; a changed durable key for a committed slot rejects the
  whole staged result.
- [ ] Engine-catalog slots (`player.*`, `session.*`, …) have no ledger
  entries, their schema identity strings stay the dotted names, and
  `is_persisted_mod_slot` filtering is unchanged. (Fingerprint bytes and
  `StateSlotId` values are expected to move: the stream version bumps, and
  ids are dense positional values over a shared engine+mod sort,
  `state_slots.rs:96-121`.)
- [ ] A Luau-only mod mints through the same tool as a TypeScript one:
  pointed at a `.luau` start-script, the mint appends the same entries it
  would for the TS twin, and the mod's durable slot commits, saves under its
  ledger key, and restores after restart — with no `scripts-build`
  involvement at any step. A hand-authored ledger passes the same path.
- [ ] A store declared with a computed name or a computed schema — invisible
  to any static read of the source — mints correctly, because the mint reads
  declarations from a real mod-init run.

## Tasks

### Task 1: Thin slice — engine-side ledger read and durable-key persistence

Build the read-only engine half end to end for the store-persistence seam,
driven by a hand-authored ledger (hand-editing is a first-class authoring
path, so the slice needs no compiler work). New module in `scripting-core`
(e.g. `crates/scripting-core/src/store_identity.rs` — it must be reachable
from the commit paths, which live in that crate, not the binary): parse
`<mod_root>/identity.json`
(`{version: 1, slots: {authoredDottedSlotName: durableKey}}`) and validate
it. Duplicate authored names must be detected **at the deserializer** — a
plain map deserialization collapses duplicate JSON keys last-wins before
any validator runs — so parse the `slots` object through a
sequence-of-pairs intermediate; then check unique keys and key grammar
`k[0-9a-f]{16}`. File rules (Orderings 18-19): unparseable or
`version != 1` rejects naming the error — never ignore-with-warning, since
loading with defaults while durable data exists reproduces the
orphaned-data failure this spec removes; an absent file is legal exactly
when the attempt declares no durable slots. The identity gate sits
**beside `plan_reconcile`, before `apply_reconcile_plan`**, in both commit
paths — `run_mod_init` (`runtime/mod_init.rs:149-160`) and the staged
commit twin (`runtime/core.rs:404-421`, apply at `:474`) — so an identity
defect rejects before any `SlotTable` mutation; a gate in the binary would
fire after `run_mod_init` has already applied. The requirement is scoped
to slots **declared in the attempt being validated**, never to the whole
live table: `apply_reconcile_plan` only inserts, and removed declarations
do not clear committed stores (`scripting.md:127`), so gating live slots
would wedge every attempt after a rename or discard until restart
(Orderings 7, 17). A durable slot — `(persist && !readonly) || network !=
ReplicationScope::None`, the same predicate as `is_persisted_mod_slot`
(`state_persistence.rs:171-175`) plus replication — missing its entry
rejects with a diagnostic naming the slot and printing a ready-to-paste
entry carrying a freshly generated key. In debug that rejection is the
staged-result rejection; in release there is no staged commit
(`runtime/core.rs:230-234`, `:295`) and `run_mod_init` failure is today
logged-and-continued (`splash_lifecycle.rs:266-267`), which would boot a
running, contentless engine — so a ledger rejection in release becomes a
fatal boot error, exit non-zero with the same diagnostic, matching the
failed-CLI-boot-load precedent (`boot_sequence.md:128`). The engine never
writes the ledger. Enforcement is a `ScriptRuntimeConfig` flag
(`runtime/types.rs:347-351`), on for every engine construction and off only
for the mint bin (Task 4), which runs this same commit path to discover
what needs minting.

Then the persistence half. Bump `CURRENT_STATE_VERSION` to 2:
`collect_persisted_state` writes bare durable keys resolved through the
ledger, `overlay_persisted_state` resolves keys back to authored dotted
names, and unknown durable keys warn and stay unapplied (Orderings 6).
Replace the bare `STATE_FILE_PATH` constant (`state_persistence.rs:16`,
resolved against the cwd at `main.rs:3837` and `splash_lifecycle.rs:328`)
with a resolver returning `<platform data dir>/postretro/<modId>/state.json`,
built the way `settings_path` builds its own (`options/mod.rs:228`) and kept
separate from load/save so tests inject a temp path instead of touching the
real user directory. Both call sites already hold the committed manifest, so
the mod id is in hand. Create the subfolder on save; a `None` from
`ProjectDirs` warns once and disables restore and save for the run, never
falls back to the cwd (Orderings 21). Saving writes through a sibling `.tmp`
and renames (`player_options.md:28`). Tests: rename-with-preserved-key
restores across a simulated restart; deleted-entry discard; missing-entry
rejection with the paste-able diagnostic and an unmutated table;
attempt-scoped gating (a live-but-undeclared slot does not reject);
duplicate-name and duplicate-key rejection, including a duplicate name that
a map parse would have collapsed; unparseable and wrong-version rejection;
absent-file legality both ways; v1 file ignored; two mod ids resolve to
distinct paths and neither save touches the other's file; no data directory
disables persistence without a cwd write; read-only mod root loads clean.
This slice falsifies the commit-ordering assumption (ledger gated beside
`plan_reconcile`, before apply and before the once-per-process overlay)
before anything fans out.

### Task 2: SDK authored-name validation, both runtimes

In `sdk/lib/data_script.ts` and `.luau`: `validateImpactEventId` becomes
single-segment — 1–64 bytes of `[A-Za-z0-9_.-]`, colon rejected — with one
shared diagnostic string in both files explaining that the engine prefixes
the mod id. `defineStore` and `defineImpactEvent` each gain a name-less
overload (first arg is the schema / filter) that throws the
direct-assignment-sugar diagnostic; the explicit-name form is unchanged and
remains the only Luau form. `defineStore` rejects `:` in the store name and
in slot names (mirror the existing `@` rejection; the engine-side twin lands
in Task 5). Wire shapes are untouched: `declaration` still carries
`{namespace, schema}` (`data_script.ts:777`) and the impact-event descriptor
still carries its authored `id`. Update the generated-typedef fixtures if
signatures surface there. Tests: TS/Luau twin parity for every new
rejection (same input class → same outcome), per `scripting.md:17`.

### Task 3: scripts-build binding-derived-name pass

In `crates/script-compiler/src/lib.rs`, add an AST visitor (pattern:
`ExportToGlobal`, `lib.rs:476+`) that matches a variable declarator whose
init is a direct call to the identifier `defineStore` or `defineImpactEvent`
whose first argument is not a string literal, and inserts the declarator's
identifier name as a new leading string argument. Fire only on a plain
identifier binding (`const x = …`, including `export const`);
destructuring, member-assignments, call results passed along, and calls in
any other expression position are left untouched. The pass is purely
syntactic — no scope analysis, no import tracking; a shadowed or re-exported
`defineStore` identifier still matches, which is acceptable because the SDK
symbols are ambient globals in script mode (`scripting.md:236`) and the
rewrite is observable in the bundle.

Hook it into `TsLoader::load` (`lib.rs:199-288`), after the parse (`:256`)
and the resolver + TS strip (`:275-276`), before the returned `ModuleData`
(`:283-287`). That is the only per-source-file site in the compiler, and it
is the one this pass needs: `bundle_with_dependencies` never sees individual
modules — it builds a `Bundler` (`:105-117`), takes one merged module back
out (`:122-130`), and every later `visit_mut_with` (`StripModuleGlue` `:136`,
`ExportToGlobal` `:142`, `DefaultExportToManifestSlot` `:148`,
`ExportSetupLevelToGlobal` `:153`, `StripExternalImports` `:155`) operates on
that merge. By then swc's inliner may have renamed top-level bindings, so the
authored binding name is gone. Two gating consequences. The same `TsLoader`
serves the prelude bundle (`lib.rs:59-68`), which must not be rewritten —
carry the discriminator the loader already has for exactly this kind of split
(`validate_sdk_imports: !prelude`, `:102`, `:196`). And it serves sources
outside the mod root: `content/dev/start-script.ts:13` imports from
`../../sdk/`. Those files are visited too, which is correct and inert — the
sugar only fills in a name the author omitted, and nothing about a file's
location determines what the mod declares. Applies to mod-init and
data-script compilation alike (both flow through the same loader).

Tests: direct-assignment rewrite; export-const rewrite; helper-internal call
NOT rewritten; explicit-name call NOT rewritten; a binding whose name survives
into the bundle only because the pass ran pre-merge; prelude bundle
unrewritten; an imported module's declaration rewritten with its own binding
name, not the importer's.

### Task 4: Identity mint bin and xtask subcommand

A new `mint-identity` `[[bin]]` in the `postretro` package
(`crates/postretro/Cargo.toml:140-146` declares the existing two), modeled on
`gen-script-types`
(`src/bin/gen_script_types.rs`): stand up `ScriptCtx`, `PrimitiveRegistry`,
and `register_all`, build a `ScriptRuntime` with identity enforcement off
(Task 1's `ScriptRuntimeConfig` flag — this is the only caller that turns it
off), call `run_mod_init(mod_root)`, and read `store_declarations` off the
stored `ModManifestResult` (`ScriptRuntime::mod_manifest`,
`runtime/core.rs:588`; `runtime/types.rs:116`). A debug build with no
start-script leaves that `None` and still returns `Ok`
(`runtime/mod_init.rs:110-116`) — the mint treats it as an error naming the
mod root, since "nothing to mint" and "no mod here" must not look alike.
Reconcile
`<mod_root>/identity.json` from those declarations: append an entry with a
fresh OS-RNG key for each durable slot — the same
`(persist && !readonly) || network != None` predicate Task 1 uses — that has
none; never modify or delete an existing line; serialize the whole file to a
sibling `.tmp` and rename over the target (`player_options.md:28`); exit
non-zero with the ledger path on any write failure, leaving the existing file
untouched. `BTreeMap` ordering keeps file diffs stable. No renderer, no
window, no session — `run_mod_init` needs none of that.

Both runtimes come free: `run_mod_init` dispatches on the start-script
extension to `run_mod_init_quickjs` (`mod_init_exec.rs:25`) or
`run_mod_init_luau` (`:374`). There is no `.ts`-specific path and no
deferral — a Luau mod mints exactly as a TypeScript one does.

Then `cargo run -p xtask -- mint-identity <mod-root>`: build the
`scripts-build` sidecar (the mint's
debug-build `run_mod_init` recompiles a stale `start-script.js` from `.ts`,
`mod_init.rs:87-100`, and needs the sidecar to do it), then shell out to the
mint with `cargo run -p postretro --bin mint-identity`, inheriting stdio and
propagating the exit code — the plumbing every other subcommand already uses
(`xtask/src/main.rs:169-197`, `:227-259`, `:330-357`). xtask parses nothing
the mint emits. Tests: mint appends once and is idempotent on re-run;
existing lines survive byte-for-byte; a computed-name and a computed-schema
declaration both mint; a Luau mod root mints the same entries as its TS twin;
unwritable mod root exits non-zero with the ledger unchanged; a mod whose
ledger is already complete writes nothing at all.

### Task 5: Mod-id qualification for impact events and name grammar

Reject `:` in mod ids inside `validate_mod_manifest_id`
(`scripting-core/src/runtime/types.rs:383-385`) — a *subtraction* from the
grammar it already enforces via the shared
`postretro_foundation::validate_ascii_identifier`
(`foundation.rs:29-51`, charset `[A-Za-z0-9_.:-]`, ≤64 bytes, non-empty).
Do **not** narrow the shared helper: it also validates ammo-type and
weapon-credit identifiers (`impact_policy.rs:436`, `combat.rs:143`, `:204`,
the js/lua reaction parsers), whose grammar keeps `:`. At impact-event
composition, form `<modId>:<authoredId>` once and use it as the composition
key and diagnostic string at all three id-keyed sites: base-filter
inheritance (`impact_policy.rs:178-186`), the per-dispatch same-id eviction
(`impact_policy.rs:219-234` — `BoundPolicy.id` carries the qualified form;
this comparison is what makes an override evict its base at fire time), and
the bind-skip / unknown-override warnings (`impact_policy.rs:190`, `:179`).
Plumbing: the rebuild path needs the committed mod id — pass it into the
impact registry alongside the active-level tags it already receives.
Engine-side store name validation adds `:` rejection in
`validate_namespace_records` (`slot_table.rs:405-429`, covering namespace
and slot names — no charset rule exists there today) and in the descriptor
parser beside the slot-name `@` rule (`store_bridge.rs:374-380`); the
engine-catalog namespaces contain no colon so `SlotTable::default` is
unaffected. Tests: override pairs and evicts across the prefix;
colon-bearing namespace and slot name rejected at both sites; mod id with
colon rejects the manifest; ammo-type ids with `:` still pass.

### Task 6: Replicated-slot schema keys by replication identity string

In `crates/postretro/src/netcode/state_slots.rs`, the schema builder
substitutes the replication identity string `<modId>:<durableKey>` for each
mod-declared slot everywhere the dotted name feeds identity today: the sort
that assigns dense `StateSlotId`s (`:89-90`, `:113`), the fingerprint
stream (`:123`, `:223+`), and the schema entry used by the id↔name apply
mapping (`:62-74` — the entry retains the authored dotted name for applying
to the local `SlotTable`, and gains the identity string as its
sort/fingerprint key). Engine-catalog slots keep their dotted names in all
three roles. Plumbing: the builder receives the ledger mapping (authored →
durable) and the mod id from the same session state that hands it the
`SlotTable`. Bump `FINGERPRINT_STREAM_VERSION` (`state_slots.rs:13-16`)
since the canonical byte stream changes. Drift-guard tests, mirroring the
`networking.md:48-50` shape: fingerprint invariant under authored rename
with preserved key; fingerprint changes under key change with preserved
name; schema rebuild after a staged reload that adds a slot retires prior
baselines exactly as today (`networking.md:56-59` behavior unchanged).

### Task 7: Ledger validation joins staged whole-attempt validation

Complete the identity gate Task 1 placed beside `plan_reconcile` in both
commit paths, on the staged path (`runtime/core.rs:404-421`): a changed
durable key for an already-committed slot rejects (Orderings 8); the
missing-entry, file-rule, and duplicate rejections re-check on every
staged attempt, always scoped to the attempt's declarations, never the
live table (Orderings 7, 17 — gating live slots would wedge every attempt
after a rename or discard until restart); orphan ledger entries and stale
entries for non-durable slots warn without rejecting (Orderings 5, 10).
The gate only reads — the reject diagnostics carry the paste-able remedy,
and no build type writes the ledger from engine code. Order of checks:
schema reconcile first (its rejections are the ones authors see most),
then identity, both before `apply_reconcile_plan` (`runtime/core.rs:474`).
Hot-reload semantics otherwise unchanged: identical redeclare preserves
values, new namespaces commit, overlap rejects — the ledger adds no new
pass over live values. Tests: each rejection class; a staged reload
carrying a renamed namespace behaves per Orderings 7; the mid-session
discard loop per Orderings 17; a ledger append landing between two staged
attempts is picked up by the second (the file is re-read per attempt); a
failed attempt leaves the `SlotTable` unmutated.

### Task 8: Content migration and dev-mod ledger

Migrate `content/dev`. Impact events: `combat-lifecycle.ts` drops the
hand-typed `dev:` prefixes from its id strings (`:19`, `:57`, `:82`; the
`.override` at `:97` needs no change — the handle carries the base id,
`data_script.ts:400-408`), and the file is genuinely in the start-script
bundle (`content/dev/start-script.ts:17-22`). Stores: `content/` declares
**no durable slot today** — no `persist` or `network` appears anywhere
under it, and the two `defineStore` files
(`coop-two-button-puzzles.ts:23`, `typed-handles-fixture.ts:52`) are
unreferenced fixtures outside the bundle whose bindings (`puzzles`,
`opts`) differ from their store names, so adopting the sugar there would
silently rename the stores — they keep their explicit-name form and stay
where they are. Instead, author one new reference durable slot reachable
from the start-script bundle (e.g. a persisted run counter in a small
`scripts/` module the start-script imports), giving ACs 1, 5, and 8 a
subject in shipped dev content; mint `content/dev/identity.json` with Task
4's tool and commit it, so the first checked-in ledger comes from the same
path authors use. Update every fixture and doc-adjacent test that asserts
the old colon-mandatory impact-id grammar or old `defineStore` arity,
including `content/dev/maps/combat-demo.README.md`'s id mentions. All call
sites and tests move in this one change, per `index.md:6`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the
ledger-read/commit/overlay ordering assumptions at the persistence seam before
anything fans out.
**Phase 2 (concurrent):** Task 2 (SDK), Task 3 (compiler sugar), and Task 5
(qualification) — independent files; Task 4 (mint bin) and Task 6 (schema
keys) — both consume Task 1's ledger module, and Task 4 additionally needs
its enforcement flag.
**Phase 3 (sequential):** Task 7 — consumes Task 1's validation hook and Task
5's grammar; owns the unified staged gate.
**Phase 4 (sequential):** Task 8 — migration; consumes every earlier task's
surface, and needs Task 4 to mint the dev mod's ledger.

## Boundary inventory

| Name | Rust | Wire / file | JS / TS | Luau | Notes |
|---|---|---|---|---|---|
| Store authored name | `StoreDeclaration.namespace` | `stores[].namespace` (unchanged) | `defineStore(name, …)` 1st arg or binding sugar | explicit 1st arg only | grammar: non-empty, no leading `@`, no `:` |
| Impact authored id | `descriptor.id` (authored) | manifest `events[].id` (unchanged field) | `defineImpactEvent(name, …)` 1st arg or binding sugar | explicit 1st arg only | `[A-Za-z0-9_.-]{1,64}`, no `:` |
| Mod id | committed manifest id | `ModManifest.id` | `defineMod({id})` | `defineMod({id})` | `[A-Za-z0-9_.-]{1,64}`, no `:` |
| Qualified impact id | composition/diagnostic key | in-memory + logs only | never authored | never authored | `<modId>:<authoredId>` |
| Durable key | ledger value; `state.json` v2 slot key (bare) | `identity.json` `slots` values; `state.json` v2 keys | never visible | never visible | `k[0-9a-f]{16}`; no mod-id prefix on disk |
| Replication identity string | schema sort/fingerprint input for mod slots | in-memory only (the 32-byte fingerprint is what crosses the wire) | never visible | never visible | `<modId>:<durableKey>` |
| Ledger file | written by the mint bin (`postretro` package); read by `scripting-core` (`store_identity.rs`) | `<mod_root>/identity.json`, `{version, slots}` — `slots` keyed by fully dotted slot names | n/a | n/a | mod content; ships with the mod; author-owned, hand-editable |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A durable key is never derived from anything mutable (random mint, recorded once, reused never) | Task 4 (mint), Task 1 (grammar validation) | re-mint on unmatched name (Orderings 5); ledger hand-edits | AC 1, 5, 6 |
| The ledger's only writer is the author's toolchain; the engine binary — every build type — only reads | Task 1 (no engine write path), Task 4 (mint bin is the sole writer) | any future engine-side "convenience" mint; the co-op fingerprint gate is what it protects (Orderings 16) | AC 7 (read-only mod root) |
| Authored ⇄ durable is injective per mod at every successful commit | Task 1, Task 7 | manual ledger merges; branch merges duplicating keys (Orderings 15) | AC 8 (duplicate-key rejection) |
| In-memory addressing stays authored-dotted-name-based; durable keys appear only at the persistence seam (bare) and the replication schema (mod-qualified) | Task 1, Task 6 | any future consumer tempted to key UI/IR by durable key | AC 10; Task 6 apply-mapping tests |
| Persisted state is scoped per mod id under the platform data dir; nothing writes `state.json` to the working directory | Task 1 (path resolver) | a `None` from `ProjectDirs` inviting a cwd fallback (Orderings 21) | AC 8 |
| Explicit-name form legal at every call site; sugar fires only on direct assignment | Task 2, Task 3 | helper-built descriptors; Luau (no compiler pass) | AC 2 |
| TS/Luau twin validation — same input class, same outcome (`scripting.md:17`) | Task 2 | the TS-only compile sugar (syntax, exempt per `scripting.md:243-245`) | AC 2, 3 |
| Whole-attempt validation: any identity defect rejects before `apply_reconcile_plan`, mutating nothing (`scripting.md:127`) | Task 1 (gate beside `plan_reconcile` in both commit paths), Task 7 (staged matrix) | a gate placed binary-side would fire after `run_mod_init` has applied; the mint's own enforcement-off construction (Task 4) | AC 6, 9 |
| The identity gate scopes to the attempt's declarations, never the live table | Task 1, Task 7 | rename/discard mid-session (Orderings 7, 17) — live-table gating wedges every later attempt | AC 9; Task 7 tests |
| Engine-catalog slots carry no ledger entries, keep dotted-name identity strings, and keep today's persistence filtering | Task 1 (entry-requirement carve-out), Task 4 (mint sees only mod-declared slots), Task 6 (keying carve-out) | schema-builder substitution | AC 10 |

## Rough sketch

- Ledger reader: `crates/scripting-core/src/store_identity.rs` — in the
  crate that owns both commit paths, so the gate can sit beside
  `plan_reconcile`; the binary's persistence code consumes the committed
  ledger through the runtime. Duplicate-name detection via a
  sequence-of-pairs deserialization of the `slots` object. Writer: the mint
  bin in `crates/postretro/src/bin/` (`BTreeMap` for stable file diffs;
  append-only; temp-then-rename). Reader and writer share one crate's format
  module — the mint links `scripting-core` like the engine does, so there is
  no second parser to drift.
- Compiler pass: new `VisitMut` in `crates/script-compiler/src/lib.rs`
  applied inside `TsLoader::load`, after the TS strip and before the
  `ModuleData` return — the only per-source-file site; `bundle_with_dependencies`
  only ever handles the merged module. Gate it off the prelude with the
  loader's existing `validate_sdk_imports` discriminator.
- Mint bin: `mint-identity` `[[bin]]` in the `postretro` package alongside
  `gen-script-types`;
  `ScriptCtx` + `PrimitiveRegistry` + `register_all`, then `run_mod_init`,
  then reconcile. Driven by an `xtask` subcommand that builds the sidecar and
  shells out.
- Qualification: mod id threading follows the path that already delivers
  `active_level_tags` to `impact_policy.rs::rebuild`.
- Schema: extend `ReplicatedSlotSchemaEntry` with the identity string; sort,
  fingerprint (`compute_fingerprint`, `state_slots.rs:223+`), and
  `id_for`/`entry_for` lookups use it; apply path keeps the authored name.

## Script syntax examples

```ts
// TypeScript — binding-derived names (compiled by scripts-build)
const progression = defineStore({
  xp: { type: "number", default: 0, persist: true },
});

const reward = defineImpactEvent({ tag: "enemy" }, (impact) => [
  { when: impact.target.healthAfter.le(0), do: [impact.source.grantAmmo("shells.buck", 8)] },
]);

// Explicit form — required inside helpers, always legal
function makeCounter(name: string) {
  return defineStore(name, { count: { type: "number", default: 0 } });
}
```

```luau
-- Luau — explicit form only (sanctioned syntax asymmetry, scripting.md:243-245)
local progression = defineStore("progression", {
  xp = { type = "number", default = 0, persist = true },
})
```

```json
// <mod_root>/identity.json — minted by mint-identity, shipped with the mod
{
  "version": 1,
  "slots": {
    "progression.xp": "k3f81c2a90d4e7b16"
  }
}
```

## Open questions

- `identity.json` filename: any collision risk with a future mod-metadata
  file the owner has planned for the mod root? Rename is free until Task 1
  lands.
- Should the replicated-slot schema fingerprint join the content-parity
  domains? Today a mismatch is caught only at snapshot apply — the batch
  drops with a `[Net]` warning while the peer stays participating
  (`netcode/state_slots.rs:802-808`, `:957-963`), and neither admission nor
  parity sees it (`mod_digest.rs:20-33`, `net/transport.rs:485-497`).
  Promoting it to a parity domain would name the divergence to the player.
  Separate spec; this one only inherits the current behavior.
