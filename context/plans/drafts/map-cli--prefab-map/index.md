# Map editing CLI and `.prefab.map` prefabs

> **Status:** unreviewed draft, ported 2026-11-23 from a requirements doc written in a private game project for the engine owner to review. Not a spec. Items for the owner to confirm are listed once, at the end. Hand-off prompt: `handoff-prompt.md` in this folder.
> **Read when:** scoping the SDK's in-place `.map` editor (`postretro-tool map`), the `.prefab.map` convention it builds its prefab registry from, and the compiler rules that check prefab instances.
> **Key invariant:** the map belongs to its human author. A tool edit survives the author's next TrenchBroom save, provided the author reopens the map; the author's edits survive the next tool run.
> **Related:** `context/lib/build_pipeline.md` (TrenchBroom `.map` → prl-build → `.prl`; §Source-format neutrality). Roadmap Epic 22, Kinematic Assemblies & Instanced Groups (`context/plans/drafts/E22--kinematic-assemblies/index.md`). `docs/modding.md` (SDK bundle contents).

## Problem

Levels are authored in TrenchBroom. Scripts and agents also need to change them: retune lights, swap textures, place or update prefab instances, rename a texture across every map. No SDK tool edits a map in place, and nothing defines a prefab across maps: TrenchBroom links groups only within one map. A project picks one of two workarounds, and both fail the author.

- **Regenerate whole.** A script writes the entire map. Any TrenchBroom edit is lost on the next run.
- **Scratch copy.** The author edits a copy; an agent reads the differences and ports them into the generator by hand. Slow, error-prone, and the copy drifts.

Evidence from one game project built on the engine (2026-11):

- 10 of its 11 maps are regenerated whole by a script; an edit in TrenchBroom is lost on the next run. The 11th was last changed in place by a retexture script.
- Its prefabs are Python functions. A new light fixture was shaped in a scratch copy of a level map, then re-expressed in code by an agent.
- A texture rename swept every map, prefab and test, but missed the scratch copy: 128 faces kept the old name.

## Goal

Three parts, shipped together in the SDK.

- **`postretro-tool map`:** a Rust subcommand that queries, checks and edits `.map` files in place, for people and agents alike. A generator becomes a scaffold run once; after hand-off, every change is a small, reviewable edit to the author's file.
- **`.prefab.map` convention:** a prefab is a TrenchBroom map whose name ends `.prefab.map`. The set of those files is the prefab registry. An instance is a linked group named for its prefab.
- **Compiler rules:** prl-build refuses an instance that does not resolve to its prefab. Ad-hoc linked groups compile as today; the tool reports them and offers fixes.

Model for the CLI: mm-ptex for Material Maker graphs (`ls`, `show`, `get`, `set`, `find`, `diff`, `check`, recipes). Each edit rewrites only the bytes it changes, verifies the result and saves atomically.

## Workflows

| Workflow | Today | With the tool |
|---|---|---|
| Retune lights in an authored level | Hand edit, or regenerate and lose edits | `set` on a selection; diff shows only those keys |
| Rename a texture everywhere | sed across files; misses copies | `rename-texture` over a directory, prefab and scratch maps included; `check` flags leftovers |
| Turn a group built in a level into a prefab | Port it into code by hand | `extract` writes a new `.prefab.map`; the group becomes its first instance |
| Edit a prefab | Edit code, regenerate every map | Open its `.prefab.map` in TrenchBroom, bump its version, run `update-prefabs` |
| Find where a prefab is used | grep | `find --prefab <name>` across the project |
| Tidy linked copies made by hand | Unnoticed | Reported when the tool opens the map; `extract` promotes them to a prefab, `unlink` separates them |
| Compare an author's sketch with the original | Compare maps by eye | `diff` reports added, moved and removed objects, in a prefab's local frame where both sides hold one |
| Scaffold a new level | Generator owns the file forever | Generator or `new` writes the first draft; the tool owns only its layer after hand-off |
| Pre-build sanity | Find out from prl-build warnings | `check` against the FGD, the project's textures and its prefabs |

## Requirements

### Round-trip fidelity

- Load and save with no edit: byte-identical for any map TrenchBroom saved. Hand-written comments are dropped, as TrenchBroom drops them.
- Edit: only touched lines change. Numbers keep their exact text; new values use TrenchBroom's style (`16`, not `16.0`).
- Output opens and saves in TrenchBroom 2026.2 with no further change: faces in TrenchBroom's order, three integer points per face where the edit allows.
- Both Standard (Quake) and Valve 220 formats.
- Comments: TrenchBroom discards them and writes its own `// entity N`, `// brush N`. The tool does the same and stores nothing in comments.
- Header: an edit sets `// Generator:` to the tool's name, so a reader can tell whether the last writer was the editor or a tool. A no-op save leaves it alone.
- Linked-group identity (`_tb_linked_group_id`) and group names are preserved exactly. The compiler's group recognition (Epic 22) reads them.
- Save is atomic. Re-parse and verify before replacing the file. Refuse if the file changed on disk between read and write.

### Addressing

- TrenchBroom renumbers entities and brushes on every save. No command takes a positional index.
- Select by: classname, key and value, targetname, group name, prefab name, layer name, texture, bounds. Selectors combine.
- A selection that matches more than one object refuses unless the command says `--all`.
- Every result prints a descriptor that selects the same object again.

### Commands

| Kind | Commands | Notes |
|---|---|---|
| Query | `ls`, `show`, `find`, `textures`, `diff` | `ls` lists layers, groups, prefab instances, entities. `find --prefab` searches the project. `diff` compares two maps object by object. |
| Edit | `set`, `unset`, `retexture`, `rename-texture`, `move`, `rotate`, `remove`, `to-layer` | `rename-texture` takes files or directories. `move` and `rotate` act on groups and point entities. |
| Prefab | `place`, `update-prefabs`, `extract`, `unlink` | See Prefabs. |
| Check | `check` | See Checks. |
| Batch | `apply <recipe>` | One `command selector args` per line. One save per run. `--dry-run` prints the diff. |
| Scaffold | `new` | Empty map with the project's game header, worldspawn and named layers. |

Shared behaviour:

- `--dry-run` on every edit prints a unified diff and writes nothing.
- `--json` on every command: deterministic order, stable field names, for agents.
- Exit status: 0 done; 1 refused or check found errors; 2 bad input. A refusal says what to change.

### Checks

| Check | Source of truth |
|---|---|
| Texture not found | The project's texture collections |
| Texture name differs only in case | Same |
| Unknown classname; unknown or ill-typed key | The FGD the project uses |
| `target` with no matching `targetname`; duplicate `targetname` | The map |
| A key TrenchBroom drops on groups and layers (the compiler already warns for `_reflection_capture`) | Compiler rules |
| Group or point entity off the grid | `--grid`, default 1 |
| Every prefab rule under Compiler rules | The prefab registry |
| Instance edited by hand since its last update; instance below its prefab's version | Instance fingerprint; the registry |

Warnings never block an edit. `check` exits 1 on errors only.

### Prefabs

**Prefab file.** A TrenchBroom map named `<name>.prefab.map`, anywhere under `content/<mod>/`; `prefabs/` is the default home. Brushes and entities sit in the prefab's local frame, pivot at the origin. It opens in TrenchBroom like any map and compiles on its own for preview.

- `<name>` is the prefab's identity and is unique per mod. Two files with one name is an error.
- Metadata lives in the prefab map's worldspawn keys, which TrenchBroom keeps on save: version (integer), description, allowed rotations, parameters.
- A parameter names an entity key inside the prefab that each instance may set: a tag, a light level, a colour. Its type and default come from the FGD entry for that key, so the prefab declares no types of its own.
- Key names and formats are the engine's to define. `// Proposed design`:

```
// worldspawn of light_wall_slot_a.prefab.map
"_prefab_version" "3"
"_prefab_description" "Wall panel with a glowing slot"
"_prefab_rotations" "0 90 180 270"
"_prefab_parameters" "slot_light:light slot_light:_tags"
```

**Registry.** The set of `*.prefab.map` files under the mod is the registry. The tool and prl-build discover it by suffix; nothing else lists prefabs.

- The tool keeps a derived index under `.build-caches/`: every prefab, and every instance across the project's maps with its version, placement and parameter values. It serves `find --prefab`, project-wide `check` and `update-prefabs --all`.
- The index is a cache. Deleting it costs speed only. Nothing that decides correctness lives in it, because authors work across machines and only git travels.

**Instance.** A TrenchBroom linked group named `prefab:<name>@<version>`, where `<name>.prefab.map` is its prefab file. TrenchBroom keeps group names on save; it drops custom keys on groups and layers, so nothing else an instance needs is stored on the group.

- Parameter values live on the instance's own entities as TrenchBroom protected properties, so linked-group propagation and updates leave them alone.
- Each instance carries a fingerprint of its contents as last written by the tool, so the tool can tell a hand edit from an out-of-date instance. It lives where TrenchBroom keeps it: a protected key on an entity inside the group. A prefab with no point entity needs a carrier; the engine decides what it is.
- Instances of one prefab in one map share a link id, so TrenchBroom propagates an author's edit across them as it does today.

**Commands.**

- `place`: writes an instance at a position and allowed rotation, with parameter values or FGD defaults.
- `update-prefabs`: rebuilds each instance from its prefab. Keeps group id, link id, placement and parameter values.
  - An untouched map comes back byte-identical.
  - An instance edited by hand since its last update is reported and left alone. `--overwrite` replaces it.
  - An instance newer than its prefab, or with a missing prefab, is reported and left alone.
- `extract`: turns a group or selection into a new `.prefab.map` at version 1, recentred on a pivot the author names. The source group becomes the first instance.
- Versions: the author bumps the prefab's version when a change should reach instances. `update-prefabs` rebuilds instances below it.
- Generated prefabs: code may write `.prefab.map` files. The tool treats them like hand-authored ones; the `// Generator:` header says which.

**Ad-hoc linked groups.** A linked group not named as a prefab instance: a linked copy an author made in TrenchBroom, say to keep two sides symmetric while working. Legitimate, and invisible to the registry.

- Every command that opens a map reports the ad-hoc linked groups in it: one notice line on stderr, each group with its member count and copies; under `--json`, a list. A notice never blocks a command or changes its exit status.
- `extract` on one copy promotes the set: writes the `.prefab.map` and renames every copy sharing its link id in that map into an instance.
- `unlink` separates the copies into ordinary groups, each with no link, as TrenchBroom's own separate command does.
- No marker for "keep it ad hoc": group keys do not survive a save, so the notice repeats while the copies stay linked.

### Compiler rules

prl-build checks prefab instances while compiling a level. Recognition stays in the `format/` adapter, per §Source-format neutrality.

| Case | Result |
|---|---|
| Group named `prefab:<name>@<v>` with no `<name>.prefab.map` | Error |
| Instance version newer than its prefab | Error |
| Instance rotation its prefab does not allow | Error |
| Two prefab files with one name | Error |
| Instance below its prefab's version | Warning |
| Ad-hoc linked group | Compiles as today, no diagnostic; the tool reports it |
- prl-build reads prefab files while compiling a level, so its build cache must account for them: renaming, deleting or re-versioning a prefab re-checks the levels that use it.
- `*.prefab.map` files are not levels. Level builds and `dist` skip them; an explicit compile of one, for preview, is allowed.
- Epic 22: an instance's prefab file is the natural shared-template identity for linked-group instancing, within a map and across maps.

### Layers and hand-off

- A scaffold writes into one named layer. After hand-off the tool rewrites that layer only, and never other layers.
- Regenerating a layer the author has edited since the tool last wrote it refuses, with a diff. `--overwrite` replaces it.
- The layer's fingerprint lives where TrenchBroom keeps it on save, as for instances.

### Editor coexistence

TrenchBroom 2026.2 does not reload a map changed on disk (owner, 2026-11-23). Its next save overwrites the tool's edit. TrenchBroom holds no lock, so the tool cannot tell reliably whether a map is open. Authors work with version control and close and reopen maps around tool runs.

- Every edit ends with a reminder naming each file written: reopen it in TrenchBroom before saving there.
- When a TrenchBroom process is running, the tool warns before writing. It does not refuse.
- `docs/` gives the loop: save and close the map in TrenchBroom, commit or stash, run the tool, review the diff, reopen.

### Packaging and scope

- A `map` subcommand of `postretro-tool`, in Rust. Ships in every SDK bundle's `bin/` and needs nothing installed, like the other `bin/` tools.
- Documented in `docs/` beside the level design guide, with worked examples. `--help` is complete on its own.
- Agent reference ships with it: commands, selectors, `--json` fields, refusals and what each asks for. Agents are a primary user.
- Game-neutral. Finds the project through `postretro.toml`; the FGD, texture collections and prefabs through the project's layout. No game names in code or defaults.
- The CLI is entirely TrenchBroom vocabulary and lives outside the compiler's shared stages. The compiler's part lives in its `format/` adapter.

## Acceptance

- Round trip over every map in the engine's `content/dev/maps` and one game project's maps: unchanged apart from hand-written comments.
- Parity with the reference behaviour: for the round trip and each edit `maplib` supports, the tool and `maplib` write identical bytes on the same input.
- An edit of each command kind, opened and saved in TrenchBroom 2026.2: no further diff. Checked by hand once per release of the tool.
- Each workflow in the table runs end to end against fixture maps in the engine's test suite.
- `update-prefabs`: an untouched map is byte-identical; a hand-edited instance is reported and unchanged.
- `extract` then `place` at the same pivot and rotation reproduces the source group's contents exactly.
- Each compiler rule fires on a fixture. A renamed prefab re-checks a cached level that uses it.
- On a fixture with ad-hoc linked groups: every command reports them; `extract` leaves the map with instances only; `unlink` leaves plain groups that TrenchBroom opens as unlinked.
- `rename-texture` over a directory leaves no old name behind; `check` reports none.
- From a fresh SDK bundle on Windows and macOS, `postretro-tool map --help` and one edit run with nothing else installed.

## Non-goals

- Editing brush geometry below the whole-brush level: no vertex moves, clips or CSG.
- Rotations other than quarter turns about the vertical on Valve 220 maps (arbitrary angles would move points off the integer grid).
- Prefabs that contain prefab instances.
- Parametric geometry: a parameter sets entity keys, never brush shapes. Projects that need shape variants write separate prefabs, by hand or from code.
- Prefabs at runtime: no PRL prefab section, no script spawning of prefabs.
- Placing prefabs from TrenchBroom's entity browser (generated FGD entries expanded by the tool). Worth revisiting once the CLI exists.
- A GUI, or replacing TrenchBroom for any task it already does well.
- Live sync with a running TrenchBroom, or a TrenchBroom plugin.
- Map formats other than Standard and Valve 220.

## Precedents to investigate

Candidates, not obligations. Check each against the invariants above before reuse.

| Precedent | What it shows | Check before reuse |
|---|---|---|
| A game project's `maplib` and `prefab_sync.py` (Python; `maplib` about 1,600 lines; in a private game repo, available from the owner) | Reference behaviour and test oracle: round trip unchanged apart from comments over the engine's `content/dev/maps`; linked groups; protected properties; prefab sync that leaves hand-edited instances alone | Its prefabs are Python builders, not `.prefab.map` files (its group names already read `prefab:<name>@<version>`); whether 2026.2 reads the protected-properties key exactly as written is unverified |
| Epic 22 group recognition (shipped) | How the compiler reads `func_group`, `_tb_group`, `_tb_linked_group_id` and layers | Where the prefab rules attach; whether its recognition already distinguishes linked from unlinked copies |
| mm-ptex (Python) | CLI shape: query and edit commands, recipes, byte-preserving `set`, refusals, atomic save | Graph semantics differ; take the interface, not the model |
| The compiler's `format/` Quake map adapter | The engine's own `.map` parser | Built to compile, not to round-trip; likely lossy for comments, number text and face order |
| `tools/migrate_neon_prefix.py` | A one-off SDK map and texture migration | `rename-texture` should cover its map half |

## Verify in TrenchBroom before build

Settled by the TrenchBroom 2026.2 manual (Linked Groups, Protected Entity Properties): protected properties exist, are set per entity in the entity inspector, block propagation in both directions, and are saved on the entity as `"_tb_protected_properties" "<key>"`. Unprotecting a key resets it to the other copies' value.

Settled by the owner (2026-11-23): TrenchBroom keeps custom keys on entities, worldspawn included, whether or not the FGD declares them. It is built to work without an FGD; the FGD only improves the editing experience. Groups and layers are the exception: they drop custom keys (engine FGD, `func_group` note).

Still to check in the editor:

- The separator when several keys are protected (the manual shows one key; the reference library writes `;` unverified).
- A protected value survives when an author edits another linked copy and TrenchBroom overwrites the group's contents.
- A `_tb_protected_properties` key written by the tool, not the editor, is read as protection on reopen.
- A group name `prefab:<name>@<v>` survives a save unchanged.
- A map saved outside `maps/` (for example `content/base/prefabs/`) finds its textures and entity models.

## Owner decisions

- Instance group name `prefab:<name>@<version>` (2026-11-23).
- No strict mode. Ad-hoc linked groups are the tool's to report and fix when it opens a map, not the compiler's to refuse (2026-11-23).
