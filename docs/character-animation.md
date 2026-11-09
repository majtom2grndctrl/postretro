# Character Animation Libraries

A character can ship as mesh and skeleton only and take its clips from a shared
clip library. A library is a glTF file with no mesh: named joints, their rest
pose, and clips. Add a clip once, and every character bound to the library has it
with no character file rebuilt.

Characters must share joint names with their library. Mixamo rigs do, including
rigs from Mixamo's auto-rigger, which often lack finger joints. A joint the
character lacks is ignored, and a joint the library lacks holds its rest pose.

## Build a library

Put the rig FBX and one FBX per clip in a folder. Each clip takes its name from
its file: `run forward.fbx` becomes `run_forward`.

```bash
blender --background --python tools/mixamo_to_gltf.py -- \
    --input-dir clips/ --base rig.fbx --library \
    --output content/mymod/models/clip_libraries/mixamo_rifle/library.gltf
```

`--base` supplies the joints; its own action is dropped. Add `--strip-face` when
the rig has facial bones that no character uses, which keeps the library smaller.

Keep the clip folder. To add a clip, place its FBX there and rerun the command.
A library rebuilds from its sources; it cannot be appended to without them.

Two options edit clips as the library is written, so a rebuild repeats them:

- `--in-place smash` holds the named clips' hips at their first position across
  the ground, keeping up and down motion. Use it on an attack that lunges or
  leaps forward: the enemy itself does not move with the clip, so it would snap
  back when the clip ended.
- `--trim smash_windup=smash:400` adds a clip named `smash_windup`: the first
  400 ms of `smash`, ending on the nearest earlier key. A state that plays it
  with `loop: false` holds its last pose. Mixamo attacks rarely hold a windup,
  so this is how an enemy holds one before it strikes. Repeat `--trim` for
  each clip. Trims are cut after `--in-place`.

Both apply to a library or to a character that keeps its own clips, never with
`--clip-library`. When the characters on a library all need the same `--yaw`,
build the library with it too, so root motion such as a lunge runs the way they
face.

## Bind a character

```bash
blender --background --python tools/mixamo_to_gltf.py -- \
    --input-dir trooper/ --base trooper.fbx \
    --clip-library ../clip_libraries/mixamo_rifle/library.gltf \
    --output content/mymod/models/trooper/model.gltf
```

`--clip-library` is relative to the model file. The converter strips the
character's clips and records the library in the document's extras:

```json
{ "extras": { "clipLibrary": "../clip_libraries/mixamo_rifle/library.gltf" } }
```

If the character faces the wrong way in game, pass `--yaw 180`. AI turns a
model's +Z toward its target.

Descriptors do not change. `clip: "run_forward"` resolves as it would for a clip
inside the model.

## How clips fit a character

The engine binds at load, with no rebake:

- **Joints match by exact name.** Each library rotation applies as a change from
  the library's rest pose, so rigs with different rest poses still share clips.
- **Proportions stay the character's.** Translation on non-root joints and all
  scale are dropped.
- **Root motion scales by height.** Root translation scales by the character's
  root rest height over the library's, so a taller character takes longer strides
  and its death poses still reach the floor.

A clip library needs clips that store a change from rest. `mixamo_to_gltf.py`
writes them that way. A clip that needs translation on a non-root joint, such as
a skinned weapon part, must stay inside the character.

## Override a clip

A character may keep clips of its own. They list first, and a clip with the same
name as a library clip wins. This is how one character gets its own `idle`.

`--clip-library` strips every clip, so it cannot produce such a character. Convert
without `--clip-library`, with only the clips you want to keep in `--input-dir`.
The base FBX's own action stays in the file too. Then add the `clipLibrary` extras
shown above to the model by hand. Rerunning the conversion discards that edit.

## When a load fails

A character stuck in its rest pose is worse than a loud error, so these fail the
model load:

- The library cannot be read.
- The library lacks the character's root joint.
- The character's root joint is unclear: its joints have several roots and the
  skin declares none, or the skin declares a joint that has a parent.
- The root's rest height is zero or below, on the character or the library.
- Neither the character nor the library has clips.

A malformed `clipLibrary` value warns, and the character loads with its own clips
only. A static model ignores a library, with a warning.

`dist` checks the models under the mod's `models/` folder. It refuses a skinned
model whose library is missing or sits outside the mod root, because that model
would work in development and break in the payload.

Keep libraries under the mod's `models/` folder, each in a folder of its own. A
rerun replaces that output's files, and a conversion with neither `--library` nor
`--clip-library` clears every model file from its output folder.

## After editing a shared clip

Weapon mounts are solved against a pose. If you change a clip a mount depends on,
`idle_aiming` by default, check each mounted character:

```bash
bin/postretro-tool solve-weapon-mount content/mymod/models/trooper/model.gltf \
  --weapon content/mymod/models/rifle/model.gltf \
  --mount-joint hand_r \
  --check
```

A rigid mount is solved against one character's hand. Characters whose hands
differ need their own bake of the weapon. See `docs/weapon-mounts.md`.

## Licence

Mixamo characters and animations may live in your project and ship in your game.
They may not be offered as downloads for other creators, and an SDK bundle copies
the mod tree whole. Keep Mixamo assets out of any bundle you publish for others to
build on.
