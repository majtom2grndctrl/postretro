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

- Mod-derived namespaces: the engine qualifies store and impact-event
  identities with the mod's identity; no declaration retypes it.
- Durable identity separate from the authored name: a per-mod identity ledger
  mapping authored slot names to minted opaque keys; persistence and the
  replicated-slot schema key by the durable form.
- The `scripts-build` binding-derived-name pass (TypeScript only, per source
  file, pre-bundle).
- Both SDK surfaces (`sdk/lib/data_script.ts` / `.luau`): new signatures and
  the revised id validation, as behavioral twins.
- Migration of existing declarations in `content/` and the `state.json`
  format version bump.

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
(`state_persistence.rs:109-114`) ignores old versions, so re-keying now costs
one version bump and nothing else. That cheapness expires when player data
ships.

A separate in-flight refactor on this branch moves pawn-to-seat ownership
onto `EntityRegistry` (netcode + entity registry files). Disjoint from this
spec's surface (`scripts-build`, SDK, store declaration/commit, persistence,
replicated-slot schema builder). Do not touch registry or netcode seat files
beyond the schema-builder keying in Task 5.

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
  pin and Task 5's fingerprint tests are the guard.
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
- **Divergence, named: the namespace root is `ModManifest.id`, not the mod
  folder name.** The settled direction said "the mod folder IS mod
  identity." Source disagrees: the manifest `id` is required, gates
  multiplayer admission, and the first committed id stays active across
  staged reloads (`scripting.md:51`); the folder is the install-local boot
  selection handle (`--mod <path>`, `boot_sequence.md:19`), and the dev mod
  already demonstrates the split — folder `content/dev/`, id
  `postretro.dev` (`content/dev/start-script.ts:26`). Data keyed by folder
  name is orphaned by reinstalling the same mod into a different folder —
  the exact defect this spec exists to remove. Deriving from `id` keeps
  every property the folder was chosen for: mod-level, unique per session by
  construction, stable under any internal file reorganization, and not the
  filename. Everything the settled direction rejected stays rejected.

**Alternatives rejected.**
- *Filename-derived namespaces* — rejected before this spec and recorded
  here: two mods can each ship `combat.ts`, and moving a file inside a mod
  would move the namespace. The wrong axis entirely.
- *Folder-derived namespaces* — rejected above: the folder is chosen by the
  installer, not the author, so it is mutable exactly where identity must
  not be.
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

## Design

**Authored names.** `defineStore(name, schema)` — the first argument becomes
the store's *authored name* (today's "namespace" argument, unchanged wire
field). `defineImpactEvent(name, filter, build)` — the authored id becomes a
**single segment**: 1–64 bytes of `[A-Za-z0-9_.-]`, no colon
(`validateImpactEventId` inverts its colon rule; the colon is now reserved
for engine qualification). Store authored names and slot names additionally
reject `:` (engine-side, beside the `@` check in
`validate_namespace_records`). The `@` rule is unchanged.

**Mod qualification.** At mod-init commit the engine forms qualified
identities `<modId>:<authoredName>` — never the author. `ModManifest.id`
gains a grammar: 1–64 bytes of `[A-Za-z0-9_.-]` (no colon), validated with
the other required-field checks. Impact events use the qualified id for
composition and diagnostics: base/override pairing and last-override-wins
selection key by qualified id (today's authored-string keying at
`impact_policy.rs:178-186` moves to the qualified form — same-mod pairs are
unaffected since both sides gain the same prefix), and the bind-skip warning
(`impact_policy.rs:190`) prints it. Impact events get **no durable key**: no
engine surface persists or replicates by event id — the id's only consumers
are in-memory composition and logs, both rebuilt every install.

**Durable identity — the ledger.** A mod ships
`<mod_root>/identity.json`: `{ "version": 1, "stores": { "<authored dotted
slot name>": "<durable key>" } }`. A durable key is minted for every
mod-declared slot that has a durable consumer — `persist: true` or
`network != None` (`slot_table.rs:43-60`) — and for no other slot (a
local-only ephemeral slot has no reader that outlives the process; no
consumer, no entry). Keys are `k` + 16 lowercase hex chars from the OS RNG,
opaque, never derived from the name or schema. Minting happens at the first
successful mod-init commit **in dev (debug) builds only**: the engine
appends the entry and rewrites the file before the commit completes; a
failed ledger write rejects the commit (an unrecorded key is a key that
changes next run). Release builds never write a mod; a persist/replicated
slot without an entry rejects the whole staged result with a diagnostic
naming the slot and the remedy (run once under a dev build). The **durable
form** of a slot is `<modId>:<durableKey>`.

**Rename and discard.** A rename is: change the name in the script *and*
edit the ledger entry's authored-name side, keeping the key. Deleting a
ledger entry is the explicit "discard that data" gesture — the next dev run
mints a fresh key and old durable rows go unmatched. An accidental rename
(script changed, ledger not) is loud, not silent: the dev build mints a new
key for the new name *and* warns that the old entry matches no declared
slot, so the recovery (merge the two lines) is visible before any release.

**Where the durable form is used.** Exactly two seams:
1. *Persistence.* `state.json` becomes version 2; slot keys are the durable
   form. Save resolves authored → durable through the ledger; the one-time
   overlay (`state_persistence.rs:105+`) resolves durable → authored. The
   existing version gate ignores v1 files with a warning — accepted loss,
   dev-only data, pre-stable.
2. *Replicated-slot schema.* Wherever the schema builder uses a mod slot's
   dotted name today — sort order for dense `StateSlotId` assignment, the
   fingerprint stream, the id↔name apply mapping
   (`netcode/state_slots.rs:64-68`, `:89-126`) — it uses the durable form
   for mod-declared slots. Engine-catalog slots keep their dotted names
   (both peers run handshake-matched engine versions; the catalog needs no
   rename protection). Both peers hold the same mod content including the
   ledger — the mod id already gates admission — so fingerprints still
   agree by content.

Everything else — `SlotTable` keys, `StateRef.slot` strings, crossing and UI
bindings, IR input leaves, HUD publisher — continues to read authored dotted
names, unchanged.

**Naming sugar.** `scripts-build` gains a second bespoke AST pass beside the
`globalThis` rewrite (`scripting.md:269`; `script-compiler/src/lib.rs:476+`),
running per source file before bundling (bundling inlines relative imports,
`scripting.md:253`, so binding context would be lost after). It rewrites a
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
| 1 | First commit of a new persist slot, dev build | Key minted and written to ledger *before* commit completes; overlay (which runs after first successful commit, once per process) sees the ledger already complete. |
| 2 | Ledger write fails (read-only mod root), dev build | Whole staged result rejected; diagnostic names the ledger path. No in-memory-only keys ever commit. |
| 3 | Release build, persist/replicated slot missing ledger entry | Whole staged result rejected; diagnostic names slot and remedy. |
| 4 | Authored rename with ledger entry updated, across restart | v2 save row (durable key unchanged) overlays into the renamed slot; fingerprint unchanged. |
| 5 | Authored rename in script only (ledger untouched), dev build | New key minted for new name + warning that the old entry matches no slot. Old save row goes unmatched (warned, retained in file until next save). |
| 6 | Ledger entry deleted, save row still present | Row is an unknown durable key at overlay: warn, ignore, slot starts at default. The documented discard gesture. |
| 7 | Mid-session staged reload renames a committed namespace | Reconciler sees delete+add by authored name: new namespace commits at defaults, removed one keeps values (`scripting.md:127` behavior, unchanged). Durable identity protects data across process runs, not across an in-session rename; overlay never re-runs (once per process). |
| 8 | Staged reload changes the durable key of a committed slot | Reject whole staged result (identity change on live state), same class as `IncompatibleSchema`. |
| 9 | Two ledger entries share one durable key | Reject whole staged result at ledger validation. |
| 10 | Ledger entry names a slot that is neither persisted nor replicated | Warn (stale grant of durability), entry retained; not an error, because removing `persist` then restoring it must not re-mint. |
| 11 | Two same-name `defineImpactEvent`s in one mod | Unchanged from today: last-registered wins per composition rules (`scripting.md:43`) — qualification adds the same prefix to both, so it neither creates nor masks the collision. |
| 12 | Sugar target shadows an SDK global (`const world = defineStore({…})`) | Rewrite fires on the binding name like any other; name validity is the SDK validator's job, not the compiler's. |
| 13 | v1 `state.json` read by the new engine | Ignored with the existing version-gate warning (`state_persistence.rs:109-114`); next clean exit writes v2. |

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
- [ ] Dev-build first commit of a new persisted slot mints a key and rewrites
  `identity.json`; a release-build commit of the same mod without that entry
  rejects the whole staged result naming the slot. A failed ledger write in
  a dev build also rejects.
- [ ] A v1 `state.json` is ignored via the version gate with a warning; a
  clean exit writes v2 with keys of the form `<modId>:<durableKey>`; a
  duplicate durable key in the ledger rejects the staged result.
- [ ] Staged hot reload: an identical redeclare with an unchanged ledger
  preserves values; a changed durable key for a committed slot rejects the
  whole staged result.
- [ ] Engine-catalog slots (`player.*`, `session.*`, …) have no ledger
  entries and their persistence filtering and schema keying are byte-for-byte
  unchanged.

## Tasks

### Task 1: Thin slice — ledger, mint, and durable-key persistence

Build the identity ledger end to end for the store-persistence seam only.
New module beside `crates/postretro/src/scripting/state_persistence.rs`
(e.g. `store_identity.rs`): parse/serialize `<mod_root>/identity.json`
(`{version: 1, stores: {authoredDottedSlotName: durableKey}}`), validate
(unique keys, key grammar `k[0-9a-f]{16}`), and mint (OS RNG). Hook the
mod-init commit path in the binary: after staged validation succeeds and
before the commit is reported successful, every mod slot with
`persist: true` or `network != ReplicationScope::None` must hold a ledger
entry — dev builds mint+append+write (write failure rejects the commit),
release builds reject with a slot-naming diagnostic. Bump
`CURRENT_STATE_VERSION` to 2; `collect_persisted_state` writes keys as
`<modId>:<durableKey>` resolved through the ledger, `overlay_persisted_state`
resolves them back to authored dotted names; unknown durable keys warn and
are ignored (Orderings 6). The mod id reaches this code from the committed
manifest (the same value `scripting.md:51` pins across staged reloads).
Tests: mint-once stability across two commits; rename-with-preserved-key
restores across a simulated restart; deleted-entry discard; duplicate-key
rejection; v1 file ignored. This slice falsifies the commit-ordering
assumption (mint before overlay; overlay once per process) before anything
fans out.

### Task 2: SDK authored-name validation, both runtimes

In `sdk/lib/data_script.ts` and `.luau`: `validateImpactEventId` becomes
single-segment — 1–64 bytes of `[A-Za-z0-9_.-]`, colon rejected — with one
shared diagnostic string in both files explaining that the engine prefixes
the mod id. `defineStore` and `defineImpactEvent` each gain a name-less
overload (first arg is the schema / filter) that throws the
direct-assignment-sugar diagnostic; the explicit-name form is unchanged and
remains the only Luau form. `defineStore` rejects `:` in the store name and
in slot names (mirror the existing `@` rejection; the engine-side twin lands
in Task 4). Wire shapes are untouched: `declaration` still carries
`{namespace, schema}` (`data_script.ts:777`) and the impact-event descriptor
still carries its authored `id`. Update the generated-typedef fixtures if
signatures surface there. Tests: TS/Luau twin parity for every new
rejection (same input class → same outcome), per `scripting.md:17`.

### Task 3: scripts-build binding-derived-name pass

In `crates/script-compiler/src/lib.rs`, add a per-file AST visitor (pattern:
`ExportToGlobal`, `lib.rs:476+`) that runs on each parsed source module
before module-glue stripping and bundling. It matches a variable declarator
whose init is a direct call to the identifier `defineStore` or
`defineImpactEvent` whose first argument is not a string literal, and
inserts the declarator's identifier name as a new leading string argument.
Fire only on a plain identifier binding (`const x = …`, including
`export const`); destructuring, member-assignments, call results passed
along, and calls in any other expression position are left untouched. The
pass is purely syntactic — no scope analysis, no import tracking; a
shadowed or re-exported `defineStore` identifier still matches, which is
acceptable because the SDK symbols are ambient globals in script mode
(`scripting.md:236`) and the rewrite is observable in the bundle. Applies to
mod-init and data-script compilation alike (both flow through
`bundle_with_dependencies`). Tests: direct-assignment rewrite; export-const
rewrite; helper-internal call NOT rewritten; explicit-name call NOT
rewritten; rewrite lands per source file before bundling merges files.

### Task 4: Mod-id qualification for impact events and name grammar

Validate `ModManifest.id` against `[A-Za-z0-9_.-]{1,64}` where the required
name/id/version checks live (`scripting-core` staged manifest validation).
At impact-event composition, form `<modId>:<authoredId>` once and use it as
the composition key and diagnostic string: base-filter registration and
override pairing (`impact_policy.rs:178-186`), the unknown-override warning,
and the bind-skip log (`impact_policy.rs:190`). Plumbing: the rebuild path
needs the committed mod id — pass it into the impact registry alongside the
active-level tags it already receives. Engine-side store name validation
adds `:` rejection beside the `@` check in `validate_namespace_records`
(`slot_table.rs:405-429`), covering both mod and future callers; the
engine-catalog namespaces contain no colon so `SlotTable::default` is
unaffected. Tests: override pairs across the prefix; colon-bearing
namespace rejected; mod id with colon rejects the manifest.

### Task 5: Replicated-slot schema keys by durable form

In `crates/postretro/src/netcode/state_slots.rs`, the schema builder
substitutes the durable form `<modId>:<durableKey>` for each mod-declared
slot everywhere the dotted name feeds identity today: the sort that assigns
dense `StateSlotId`s (`:89-90`, `:113`), the fingerprint stream (`:123`,
`:223+`), and the schema entry used by the id↔name apply mapping (`:64-68`
— the entry retains the authored dotted name for applying to the local
`SlotTable`, and gains the durable form as its identity/sort/fingerprint
string). Engine-catalog slots keep their dotted names in all three roles.
Plumbing: the builder receives the ledger mapping (authored → durable) and
the mod id from the same session state that hands it the `SlotTable`. Bump
the fingerprint version prefix constant (`state_slots.rs:13-15`) since the
canonical byte stream changes. Drift-guard tests, mirroring the
`networking.md:48-50` shape: fingerprint invariant under authored rename
with preserved key; fingerprint changes under key change with preserved
name; schema rebuild after a staged reload that adds a slot retires prior
baselines exactly as today (`networking.md:56-59` behavior unchanged).

### Task 6: Ledger validation joins staged whole-attempt validation

Extend the staged-commit gate so identity defects reject the whole staged
result, symmetric with `plan_reconcile` (`slot_table.rs:255-299`): duplicate
durable keys; a changed durable key for an already-committed slot (Orderings
8); missing entries per build type (dev mints here — Task 1's hook is this
gate's first half); orphan ledger entries and stale entries for
non-durable slots warn without rejecting (Orderings 5, 10). Order of checks:
schema reconcile first (its rejections are the ones authors see most), then
identity. Hot-reload semantics otherwise unchanged: identical redeclare
preserves values, new namespaces commit, overlap rejects — the ledger adds
no new pass over live values. Tests: each rejection class; a staged reload
carrying a renamed namespace behaves per Orderings 7.

### Task 7: Content migration and dev-mod ledger

Migrate `content/dev`: `coop-two-button-puzzles.ts:23` and
`typed-handles-fixture.ts:52` drop their explicit store-name strings in
favor of the binding sugar (bindings already carry the right names);
`combat-lifecycle.ts:18,56,81` drop the hand-typed `dev:` prefixes (the
`.override` at `:97` needs no change — the handle carries the base id,
`data_script.ts:400-408`). Commit the minted `content/dev/identity.json`
produced by a dev run (the `coopPuzzles` slots are the only
persist/replicated candidates today — verify against their schemas at
migration time). Update every fixture and doc-adjacent test that asserts
the old colon-mandatory impact-id grammar or old `defineStore` arity,
including `content/dev/maps/combat-demo.README.md`'s id mentions. All call
sites and tests move in this one change, per `index.md:6`.

## Sequencing

**Phase 1 (sequential):** Task 1 — thin slice; falsifies the
mint/commit/overlay ordering assumptions at the persistence seam before
anything fans out.
**Phase 2 (concurrent):** Task 2 (SDK), Task 3 (compiler), Task 4
(qualification) — independent files; Task 5 (schema keys) — consumes Task
1's ledger module.
**Phase 3 (sequential):** Task 6 — consumes Task 1's mint hook and Task 4's
grammar; owns the unified staged gate.
**Phase 4 (sequential):** Task 7 — migration; consumes every earlier task's
surface.

## Boundary inventory

| Name | Rust | Wire / file | JS / TS | Luau | Notes |
|---|---|---|---|---|---|
| Store authored name | `StoreDeclaration.namespace` | `stores[].namespace` (unchanged) | `defineStore(name, …)` 1st arg or binding sugar | explicit 1st arg only | grammar: non-empty, no leading `@`, no `:` |
| Impact authored id | `descriptor.id` (authored) | manifest `events[].id` (unchanged field) | `defineImpactEvent(name, …)` 1st arg or binding sugar | explicit 1st arg only | `[A-Za-z0-9_.-]{1,64}`, no `:` |
| Mod id | committed manifest id | `ModManifest.id` | `defineMod({id})` | `defineMod({id})` | `[A-Za-z0-9_.-]{1,64}`, no `:` |
| Qualified impact id | composition/diagnostic key | in-memory + logs only | never authored | never authored | `<modId>:<authoredId>` |
| Durable key | ledger value | `identity.json` `stores` values | never visible | never visible | `k[0-9a-f]{16}` |
| Durable slot form | persistence + schema key | `state.json` v2 keys; schema fingerprint stream | never visible | never visible | `<modId>:<durableKey>` |
| Ledger file | new module in `postretro` binary | `<mod_root>/identity.json`, `{version, stores}` | n/a | n/a | mod content; ships with the mod |

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| A durable key is never derived from anything mutable (random mint, recorded once, reused never) | Task 1 (mint) | dev re-mint on unmatched name (Orderings 5); ledger hand-edits | AC 1, 5, 6 |
| Authored ⇄ durable is injective per mod at every successful commit | Task 1, Task 6 | manual ledger merges; branch merges duplicating keys | AC 7 (duplicate-key rejection) |
| In-memory addressing stays authored-dotted-name-based; durable form appears only at persistence and replication seams | Task 1, Task 5 | any future consumer tempted to key UI/IR by durable form | AC 9; Task 5 apply-mapping tests |
| Explicit-name form legal at every call site; sugar fires only on direct assignment | Task 2, Task 3 | helper-built descriptors; Luau (no compiler pass) | AC 2 |
| TS/Luau twin validation — same input class, same outcome (`scripting.md:17`) | Task 2 | the TS-only compile sugar (syntax, exempt per `scripting.md:243-245`) | AC 2, 3 |
| Whole-attempt staged validation: any identity defect rejects everything, mutates nothing (`scripting.md:127`) | Task 6 | mint-write ordering (Orderings 1-2) | AC 6, 8 |
| Engine-catalog slots carry no ledger entries and are byte-identical in schema and persistence | Task 1 (mint filter), Task 5 (keying carve-out) | schema-builder substitution | AC 9 |

## Rough sketch

- Ledger: `crates/postretro/src/scripting/store_identity.rs`; serde_json like
  `state_persistence.rs`; `BTreeMap` for stable file diffs.
- Compiler pass: new `VisitMut` in `crates/script-compiler/src/lib.rs`
  applied where each module is parsed inside `bundle_with_dependencies`,
  before `StripModuleGlue`.
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
// <mod_root>/identity.json — minted by a dev-build run, shipped with the mod
{
  "version": 1,
  "stores": {
    "progression.xp": "k3f81c2a90d4e7b16"
  }
}
```

## Open questions

- Should an `xtask mint-identity` command exist for authors who never run
  dev engine builds, or is "run once under a dev build" sufficient for the
  foreseeable modder workflow? Owner call; the release-build rejection
  diagnostic can name either remedy.
- `identity.json` filename: any collision risk with a future mod-metadata
  file the owner has planned for the mod root? Rename is free until Task 1
  lands.
