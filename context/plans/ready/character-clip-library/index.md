# character-clip-library

Brief · compact · reads: `context/lib/resource_management.md` §7, §8.2 · `context/lib/rendering_pipeline.md` §9 · read at a82073138

## Problem

Owner-raised capability, needed before the enemy roster grows. Every character glTF embeds its own copy of its clips, so each new character re-imports the whole clip set, and a new clip reaches a character only through a full per-character rebuild. When done, a character ships as mesh and skeleton and takes its clips from a shared library. Adding a clip is one library edit. Adding a Mixamo-rigged enemy imports no clips. Measurements: `research.md`.

## Decisions

- **Library format.** A clip library is a mesh-less glTF: named joints, their rest TRS, and clips. It keeps the Blender exporter and the `clip` name strings. A custom binary format buys nothing yet; `roadmap-q3-archive.md` Epic 10 already defers a per-mesh bake to measured need.
- **Reference lives in the model.** Document-level `extras.clipLibrary` names one library by a path relative to the model document. `data:`, `file:` and rooted values are malformed, matching `gltf_resolve.rs`. The model handle stays a complete identity, so level prediction, handle-keyed clip tables and hit-zone stores, and the SDK do not change. Rival: a descriptor field lets one mesh swap libraries, but costs a modder-facing field and per-(mesh, library) identity.
- **Bind at load, inside the model loader.** Binding yields ordinary per-model clips in the mesh's joint order. Runtime consumers stay library-unaware, and every `load_model` caller gets bound clips. CPU clip memory stays per model type, as `rendering_pipeline.md` §9 accepts.
- **Divergence from "no import-time normalization".** `resource_management.md` §7 and `plans/done/weapon-mount-frame-solver` keep bone conversion out of the loader. Binding narrows that rule: a model's joint and rest data stay raw, and only library tracks are composed with the mesh's rest. Load time wins over a tool-time splice because a splice leaves stale state after every library edit and keeps clips in each character file. §7 names this and the loader's existing root-motion rewrite of a model's own tracks.
- **Bind contract: rest-relative rigs only.** A library track means a delta from the library's rest, which is what `mixamo_to_gltf.py` writes. A rig whose clips are not rest-relative cannot use a library.
- **Bind math.** Joints match by exact name. Rotation is `rest_mesh * rest_lib⁻¹ * q`. Root translation is `rest_mesh + C * (t - rest_lib) * k`, where `C` is the root's rest correction and `k` is mesh root rest height over library root rest height. Non-root translation and all scale are dropped, so proportions belong to the mesh. A mesh joint with no library joint holds rest; a library joint with no mesh joint is ignored. `k` replaces `limitator`'s conversion `--scale` on purpose, which changes that character's root motion.
- **Mechanism vs policy.** The engine owns the bind math and names no rig: no `mixamorig`, no bone vocabulary. `tools/mixamo_to_gltf.py` owns Mixamo policy, including normalizing numbered `mixamorig` prefixes to the canonical one. Joint names become a content contract between a library and its meshes.
- **Embedded clips stay valid.** The clip list is embedded clips, then library clips in library order. A name in both takes the embedded clip with no warning: that is how a character overrides a shared clip. Order stays defined because an animated state whose clip does not resolve falls back to clip 0. The KayKit knight is untouched.
- **Failure policy.** These are model load errors, like an unreadable buffer, because a character silently stuck in rest pose is the worse outcome: an unreadable library; a mesh root joint absent from the library; a skinned mesh for which the loader selects no locomotion root, since that selection defines the root for binding; a root rest height of zero or below on either side, where height is the root's local rest Y; a library with no clips on a model with no embedded clips. A malformed `clipLibrary` value warns and reads as absent, as malformed sockets do. A static model naming a library warns and ignores it.
- **`dist` checks the reference.** It refuses a skinned model whose well-formed `clipLibrary` is missing or resolves outside the mod root: an out-of-mod library works in dev and breaks in the payload. Static models and malformed values load anyway, so `dist` skips them. Library buffers stay unchecked, as model buffers are.
- **Cache identity.** Library bytes join the model content key and the key version bumps. No path enters the key (`resource_management.md` §8.2).
- **Migration ships here.** `exo_red` and `limitator` become mesh-only consumers of one library, built by a real `--library` run so it has a recipe. Library clips start at time zero, which removes `exo_red`'s one-frame lead.
- **Libraries rebuild from source.** A new clip means placing its FBX in the library's clip folder and rerunning `--library`; bound characters are not re-baked. Clip FBXs stay outside the repo. Appending to a library without its sources is not offered. Editing a shared clip stales weapon mounts solved against it, so the docs name `solve-weapon-mount --check`.
- **Two new characters prove the path.** `rodin_sci-fi_trooper_m` and `cyberpunk_warrior_f`, rigged by Mixamo's auto-rigger on its no-fingers skeleton, ship as mesh-only models. Neither has had these clips and both lack the library's finger joints, so they test what migration parity cannot. Their rigged meshes replace the committed static ones; `research.md` names the sources.
- **Scenery stays; enemies are new classnames.** `sci_fi_trooper_m` and `cyberpunk_warrior_f` keep their `campaign-test` placements and gain an idle state with no behavior, because a skinned model on a stateless entity draws a T-pose. New enemy classnames share those models, each with a faction of its own.
- **Factions get room.** `movement-feel` and `stress-warren-mini` gain more enemies across more factions. Each faction holds its own area of the map, with space between factions, so groups meet rather than spawn mixed. Existing `movement-feel` placements move where they break this.
- **Non-goals.** IK, general humanoid retargeting, keyframe compression, sharing clips between rigs whose joint names differ, and layering several libraries on one model, since a character's extra clips embed. Non-humanoid enemies embed clips or get their own library. Clip-fit issues, such as the support hand leaving the rifle, stay: binding reproduces today's poses and does not improve them. Non-root translation cannot ride a library; a clip that needs it, such as a skinned-weapon joint, embeds. Peer parity of library bytes stays out: clips already pose the hit zones behind client hit declaration with no digest over them, and a library adds a file to that gap rather than creating it.

### Authoring surface

Normative for names and calling pattern. The `--library` and `--clip-library` flags are new. Runnable from an SDK bundle.

```bash
# Build a library from a folder of clip FBXs. The rig FBX sits inside that folder.
# To add a clip, place its FBX there and rerun.
blender --background --python tools/mixamo_to_gltf.py -- \
    --input-dir clips/ --base rig.fbx --library \
    --output content/mymod/models/clip_libraries/mixamo_rifle/library.gltf

# Convert a character: mesh and skeleton only, bound to that library.
blender --background --python tools/mixamo_to_gltf.py -- \
    --input-dir trooper/ --base trooper.fbx \
    --clip-library ../clip_libraries/mixamo_rifle/library.gltf \
    --output content/mymod/models/trooper/model.gltf
```

```json
{ "extras": { "clipLibrary": "../clip_libraries/mixamo_rifle/library.gltf" } }
```

Descriptors do not change: `clip: "run_forward"` resolves as before.

## Acceptance

### Automated

Binding, on fixture rigs:
- [ ] A mesh with a different joint order and joint count from its library poses each named joint from the library clip, and no joint from another joint's track.
- [ ] Rigs with different rest rotations produce the corrected rotation; a fixture where uncorrected name-matching gives a different pose pins this.
- [ ] A mesh joint missing from the library holds rest. A library joint missing from the mesh is ignored and the load succeeds.
- [ ] Bone lengths stay the mesh's when the library moves or scales a non-root joint, and a library scale track on the root leaves the mesh root at its rest scale.
- [ ] Root translation scales by the height ratio and turns into the mesh's facing. A library root at rest leaves the mesh root at its own rest.
- [ ] A library clip with horizontal root travel derives travel speed scaled by the height ratio and neutralizes as an embedded clip does. A clip with no travel derives none.
- [ ] Animation bounds and hit-zone reach cover a pose reached only through a library clip; with a height ratio above 1, a clip raising the root beyond any other pose widens them to the scaled height (P4).
- [ ] A library with no skin, its joints under a non-joint parent, binds: its root is the node named as the mesh's root, not the document's top node.
- [ ] A bound track keeps its library key times and interpolation mode, and a clip has one duration on every mesh that binds it, whichever joints or channels that mesh drops (P5).

Clip list:
- [ ] Embedded and library clips both resolve by name, embedded first; a name in both yields the embedded clip and no warning.
- [ ] In a model with embedded and library clips, each embedded clip keeps the tracks and travel speed it has with no library (P6).
- [ ] With no embedded clips, clip 0 is the library's first clip.
- [ ] A model with no `clipLibrary` loads exactly as it does today (regression guard).

Refusal and permission:
- [ ] Each fails the load: an unreadable library, including one not glTF or missing a buffer; a library lacking the mesh's root joint; a library named by a skinned mesh with no selected root, such as an undeclared forest of joints or a declared non-root; a root rest height of zero or negative, on mesh or library. A forest with a declared root binds.
- [ ] A library with no clips fails the load of a model with no embedded clips. With embedded clips, the model loads.
- [ ] A non-string, empty or rooted `clipLibrary`, or one containing a colon after percent-decoding such as `file:`, `http://x` or `C:lib.gltf`, warns, the model loads with embedded clips only, and the document's `tags` survive. A relative path containing `..` is accepted.
- [ ] A static model naming a library warns and loads, even when that library is unreadable, and editing that library leaves the model's content key unchanged (P3).

Cache and shipping:
- [ ] Changing one byte of a library's document, or of a buffer it references, changes the model's content key (P1). Moving a model and its library together, with their relative path unchanged, keeps it.
- [ ] A library edited or deleted after a model's sources are read leaves that load unchanged: the parse binds the bytes the key covered (P2).
- [ ] A cache hit and a fresh load produce the same clips, compared by track and not only by name and duration, including across a level change that keeps the model. The run sets `POSTRETRO_REQUIRE_GPU`.
- [ ] A library edited between two levels makes the kept model miss, while the old level's set keeps drawing the old entry until release (P8).
- [ ] `dist` completes on a mod with a library under `models/`, the payload holds the library's document and buffer, and a bound model loaded from that payload tree has its library clips.
- [ ] `dist` refuses a skinned model whose `clipLibrary` is missing, or names an existing file outside the mod root through `..`. It accepts a nested model whose `..` leaves `models/` but stays in the mod, and skips a static model or a malformed value.
- [ ] The authoring-surface commands' post-export pass stamps `clipLibrary`, strips clips from a character, and normalizes a numbered `mixamorig` prefix.
- [ ] A numbered-prefix rig leaves the tool with the same pose masks, sockets, hit zones and skin root as a canonical rig, and `--no-tag` still stamps `clipLibrary` (P7). A numbered-prefix clip FBX with a canonical rig keeps its clip.
- [ ] The post-export pass tests run under `python -m unittest discover -s tools/tests` with Blender stubbed, and the run's test count includes them.

Layering:
- [ ] Grep gates: no engine source names `mixamorig`; binding fixtures use invented joint names; sampler, hit-zone, clip-table and netcode sources carry no library reference.

Migration:
- [ ] `exo_red` and `limitator` embed no clips, and every clip name a dev descriptor references resolves.
- [ ] Neither keeps an accessor, buffer view or buffer byte only clips used: `.gltf` plus `.bin` total under 1.5 MB and 0.3 MB. The post-export pass leaves a character the same way.
- [ ] One-time report, kept in the plan of record: migrated models loaded through the engine, compared with bakes read from the recorded pre-migration commit. `limitator`'s rotations match at the same clip time across every library clip, including the ones only it carried today. `exo_red`'s rotations match with its baseline one frame later. Rotations stay within 0.25°, and `exo_red`'s root translation within 1 mm (P9).
- [ ] One-time report: each migrated model's mesh, skeleton, inverse binds, sockets, hit zones and pose masks equal its load at the recorded pre-migration commit.
- [ ] Every clip in the migrated library has its first key at time zero, and the plan of record holds the `--library` command and source folder listing that built it.
- [ ] `limitator`'s `death_from_front_headshot` ends with its lowest joint within 2 cm of the floor, and `idle_crouching` likewise.
- [ ] `solve-weapon-mount --check` for AR_4 on `limitator` reports the barrel and up metrics recorded at the pre-migration commit, within 0.001. Status UNVERIFIED is accepted: AR_4 declares no axes.

New characters and docs:
- [ ] The `rodin_sci-fi_trooper_m` and `cyberpunk_warrior_f` model files each have a skin, embed no clips and name the library. Each model folder keeps `license.txt` and no longer holds the static `scene.gltf`, `scene.bin` or its texture.
- [ ] `sci_fi_trooper_m` and `cyberpunk_warrior_f` declare an idle state whose clip resolves and no behavior, and `campaign-test.map` is unchanged.
- [ ] Each new enemy classname declares animation states whose clips all resolve, a faction no other classname uses, and a hitbox reaching its character's height. The dev-map enemy tag guard lists it and passes.
- [ ] `movement-feel` and `stress-warren-mini` each place more enemies and more factions than before.
- [ ] `solve-weapon-mount --check` passes for each new enemy with the rifle its descriptor mounts (P10). Status UNVERIFIED is accepted. A separate baked rifle per character is allowed; the shared AR_4 keeps its `limitator` metrics.
- [ ] The character animation page in `docs/` exists, each command on it names only paths a bundle ships, and it names `solve-weapon-mount --check` as the follow-up to editing a shared clip.

### Manual

- [ ] Player and `limitator` walk, run, aim, fire and die as before. `limitator`'s corpse rests on the floor.
- [ ] On `movement-feel`, each new enemy turns its front to its target, walks, fires with the barrel toward that target, and dies, with no clips in its file. Support-hand fit is not measured.
- [ ] On `movement-feel` and `stress-warren-mini`, each faction starts in its own area with visible space to the next, and no two factions start mixed.
- [ ] The `campaign-test` trooper and warrior props play idle and never show a T-pose.
- [ ] In real Blender, the `--library` command on a rig and at least two clip FBXs writes a mesh-less library with named joints and clips, none from the base's own action, and a character bound to it plays those clips in game. A rerun with one more clip FBX adds that clip to every bound character with no character file changed.

## Path

- Seams: `read_model_sources` and `ModelSources` (library bytes and digest ride here, so `model_content_key` sees them and a miss reads no file twice); `load_model_from_sources`; `build_skeleton`, whose `node_to_topo` is dropped after load, so name lookup must happen while nodes are live. `Joint` needs no name field.
- `load_clip` runs root-motion derivation per clip. Binding must transform a parsed clip before that step, so split derivation from parsing.
- `gltf_loader.rs` is past 800 lines. Move clip parsing to its own module first, behavior-preserving, own commit. Binding gets a new module beside it.
- `model_cache_tests` and `lifecycle_model_sweep` compare products against `load_model`; they should hold unchanged and are the first alarm for key drift. `dev_models` walks every `.gltf` under `content/dev/models`, so a dev library enters that walk and returns `NoMesh` on both sides.
- A mesh-less library cannot pass `read_model_sources` or `select_model`, so it needs its own read. Skin presence is known at read time, so the static-model check can precede it.
- Stage 4 of `dist` walks every glTF under `models/`. `resolve_document_base_color_paths` only loops materials, so a mesh-less library should pass; the row above confirms it.
- Strongest rival, a tool-time bake: the same bind math splices clips into each character's glTF and the loader stays untouched. It keeps §7 whole, but needs a glTF splice writer and a staleness gate in `dist`, keeps megabytes of clips per character, and misses "ships as mesh and skeleton".
- First slice: bind a scratch library onto `rodin_sci-fi_trooper_m` and look at it. The `limitator` diff is only a regression check, because `mixamo_to_gltf.py` wrote the deltas the formula assumes. If the trooper fails, stop and revisit the brief.
- Record the pre-migration commit in the plan of record; undoing the migration gets dearer with each later mesh-only character.
- Build the migrated library from `exo_red.fbx`, the Pro Rifle Pack, and only the Basic Shooter clips `limitator` uses (`firing_rifle`, `hit_reaction`, `reloading`); the script takes one `--input-dir`, so stage them together. Stripping clips from the existing character glTFs avoids re-solving mounts; re-converting is the alternative. The one-time mesh report covers either.
- Stage 4 of `dist` already walks every glTF under `models/`, so the reference check can sit there.
- `stress-warren-mini` is a gate probe with recorded baselines in `stress-warren.README.md`, and `movement-feel.README.md` documents its placements. Re-record or update both, and recompile both maps. Faction ids and new classnames are the executor's; `faction-crossfire.ts` is the precedent.
- `docs/` has no character animation page. Add one beside `weapon-mounts.md`; every command must run from a bundle.

## Open questions

- Whether to parse a shared library once per load rather than once per model — **delegated**: measure on a level with every bound character, report in the plan of record.
- Policy when a bound joint's parent differs between mesh and library, and how to surface unbound joints without warning on every leaf — **delegated**: report the choice. A renamed non-root joint otherwise holds rest silently.
