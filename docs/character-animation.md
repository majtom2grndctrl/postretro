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

Three options edit clips as the library is written, so a rebuild repeats them:

- `--in-place smash` holds the named clips' hips at their first position across
  the ground, keeping up and down motion. Use it on an attack that lunges or
  leaps forward: the enemy itself does not move with the clip, so it would snap
  back when the clip ended.
- `--trim smash_windup=smash:400` adds a clip named `smash_windup`: the first
  400 ms of `smash`, ending on the nearest earlier key. A state that plays it
  with `loop: false` holds its last pose. Mixamo attacks rarely hold a windup,
  so this is how an enemy holds one before it strikes. Repeat `--trim` for
  each clip. Trims are cut after `--in-place`.
- `--trim smash_strike=smash:400-` adds the rest of `smash` from 400 ms, starting
  on the same key a `smash:400` windup ends on, so a strike played after its
  held windup starts from the held pose. `--trim name=clip:400-900` stops at
  900 ms instead, on the nearest earlier key. The copy's hips start where the
  source's hips start along the ground, because the engine keeps every clip's
  first and last hip position over the character. A windup that steps back
  therefore holds with its hips over the character, and its strike starts
  there too. An enemy plays the pair as a stance and its strike: the stance's
  `animation` plays the windup, and the attack's `release` plays the strike,
  landing on its `hit` event. See
  [Stances](scripting-reference.md#stances).
- `--event punch:hit=620` names a moment in a clip: `hit`, 620 ms into `punch`.
  An enemy attack can fire on it. The clip stores it in its extras, in seconds:

  ```json
  { "name": "punch", "extras": { "events": [{ "name": "hit", "time": 0.62 }] } }
  ```

  Every character on the library shares the clip's events. A name starts with a
  letter or underscore and holds only letters, digits and underscores, and case
  counts: `hit` and `Hit` are two events. Repeat `--event` for each one. The
  converter refuses an event past its clip's end, a name used twice in one clip,
  and a clip it cannot find.

  Events are added before trims are cut, and each trim keeps the events inside
  it. A range shifts them by its start key: with `hit` at 767 ms,
  `--trim punch_strike=punch:467-` starts on the key at 466.7 ms and has `hit`
  at about 300 ms. An event on a range's start key lands at 0 in that range, not
  in a windup ending on that key, so a windup keeps only the events before its
  last pose.

All three apply to a library or to a character that keeps its own clips, never
with `--clip-library`.

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

The converter's default output faces +Z, the front of every model in the engine:
AI turns a model's +Z toward its target. A library's root motion follows the
character it binds to, so the library needs no matching `--yaw`.

Descriptors do not change. `clip: "run_forward"` resolves as it would for a clip
inside the model.

## How clips fit a character

The engine binds at load, with no rebake:

- **Joints match by exact name.** Each library rotation applies as a change from
  the library's rest pose, on top of the character's own rest pose. Small
  differences in rest pose carry through as small offsets. A large one does not
  fit: an A-pose character on a T-pose library swings its arms across its body.
  See [Match the library's rest pose](#match-the-librarys-rest-pose).
- **Proportions stay the character's.** Translation on non-root joints and all
  scale are dropped.
- **Root motion scales by height.** Root translation scales by the character's
  root rest height over the library's, so a taller character takes longer strides
  and its death poses still reach the floor.

### Match the library's rest pose

Mixamo's own downloads are posed for each character, so this comes up with a
rigged character from elsewhere: a Sketchfab model with Mixamo-named joints, for
example, often binds in an A-pose. Re-rest it onto its library's pose after
binding:

```bash
python3 tools/rest_to_library.py content/mymod/models/ghoul/model.gltf
```

Each joint takes the library's rest rotation, and the mesh moves into that pose
with its own skin weights. The root joint keeps its own facing: it takes the
library root's tilt but not its turn, so a character facing +Z stays +Z on a
library that faces −Z. The character's bone lengths stay its own. The script
needs Python with `numpy` but not Blender, and rewrites the model in place.
Rerunning the conversion undoes it, so run it again after any reconversion.

A clip library needs clips that store a change from rest. `mixamo_to_gltf.py`
writes them that way. A clip that needs translation on a non-root joint, such as
a skinned weapon part, must stay inside the character.

## Attack clips for gun-wielding enemies

An enemy's shot leaves its weapon's muzzle, posed on the clip it is playing at
the game tick it fires, not at the render frame. Pick or cut an attack clip that
holds the gun toward the target at the moment it fires: a clip that swings the
gun through the aim fires wherever the gun is at that tick. A held windup like
`smash_windup` cut with `--trim` is the simplest way to get a steady aim. Watch
the shots in the level, and see [Weapon mounts](weapon-mounts.md) for the `muzzle` socket.

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
  --weapon content/mymod/models/rifle/trooper.gltf \
  --mount-joint hand_r \
  --check
```

A rigid mount is solved against one character's hand, so each rig has its own
bake of each weapon, named for the rig: `rifle/trooper.gltf` above fits the
`trooper` model. A character lists its bakes in `mesh.mounts`. Check every
bake a changed clip affects. See `docs/weapon-mounts.md`.

## Licence

Mixamo characters and animations may live in your project and ship in your game.
They may not be offered as downloads for other creators, and an SDK bundle copies
the mod tree whole. Keep Mixamo assets out of any bundle you publish for others to
build on.
