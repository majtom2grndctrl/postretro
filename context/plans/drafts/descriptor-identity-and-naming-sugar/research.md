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
| `globalThis.<name>` rewrite is a bespoke post-bundle AST pass | `context/lib/scripting.md:269`; `crates/script-compiler/src/lib.rs:88-93`, `:134-142` (`ExportToGlobal` ordering), `:476+` | Confirmed. `bundle_with_dependencies` visits each parsed module — a second per-file pre-bundle pass has a natural home there. |
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
    participant B as scripts-build (--mod-root)
    participant L as identity ledger (mod root)
    participant S as start-script (VM)
    participant C as mod-init commit
    participant T as SlotTable (memory)
    participant P as state.json

    B->>L: reconcile: append keys for new durable slots<br/>(append-only; write failure fails the compile)
    S->>C: ModManifest.stores (authored names)
    C->>L: read + validate (engine never writes)
    alt persist/replicated slot has no entry (any build type)
        C-->>S: reject whole staged result,<br/>print paste-able entry
    end
    C->>T: commit slots keyed by authored dotted name
    Note over C,P: once per process, after first successful commit
    P->>C: v2 doc keyed by modId:durableKey
    C->>T: overlay via ledger (durableKey → authored name)
    Note over T,P: clean exit: collect persisted mod slots,<br/>write v2 keys via ledger
```

Read call sites for every arrow: staged commit (`scripting-core/src/staged_manifest.rs` drain), overlay/save (`state_persistence.rs:68-99`, `:105+`; gating `StateStoreLifecycle` `:20-36`), replication schema build (`netcode/state_slots.rs:89-126`).

## Owner rulings (2026-08-08) — verification

Two rulings folded into the spec; both relayed citations re-verified here
rather than taken on trust.

1. **Folder → `ModManifest.id` reversal ratified.** Spec keeps the id-rooted
   namespace; filename- and folder-derivation both stay recorded as rejected
   with reasons.
2. **Minting moved to the author's toolchain (`scripts-build`); the engine
   only reads the ledger, every build type.** Load-bearing argument
   verified: `crates/postretro/src/netcode/state_slots.rs:78-79` — "Both
   peers build this identically from their own slot tables; a fingerprint
   match is the cross-peer agreement gate" — so per-install minting lets the
   same mod diverge against itself; and `context/lib/networking.md:81-83` —
   "a parity mismatch **never closes the connection**… Content divergence is
   a diagnostic to a still-connected peer, not a disconnect" — so the
   failure is quiet, which is worse. Supporting claims verified:
   `scripting.md:257` (xtask builds the sidecar first — mandatory build
   step) and `scripting.md:35` (store declarations reach the engine only via
   the manifest, i.e. the start-script bundle — so the start-script compile
   sees every durable declaration that is statically visible).

**Watcher self-trigger hazard, resolved precisely.** The hazard does not
disappear at the watch layer — the mod root is watched non-recursively
(`crates/scripting-core/src/watcher.rs:328-338`), so a ledger append does
emit an event. It disappears at the classification layer: reload
classification is membership in the tracked mod-init dependency set
(`crates/scripting-core/src/staged_manifest/transfer.rs:59-64`; dependencies
collected from the TS compile's dependency report,
`staged_manifest.rs:228-297`), and `identity.json` is never a script
dependency. Pinned as Orderings row 14. Secondary containment: reconcile
writes only when a new durable slot appeared, i.e. when a script edit
already triggered the reload in progress.

**Limit stated honestly in the spec:** `scripts-build` never sees Luau
sources (`scripting.md:259` — Luau passes through unchanged) and cannot
read computed schemas, so build-time minting covers statically visible TS
declarations only; the engine's paste-able missing-entry diagnostic plus
hand-editing (the ledger is author-owned JSON) covers the rest.

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
