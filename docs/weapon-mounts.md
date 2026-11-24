# Rigid Weapon Mount Workflow

Use this deterministic workflow to fit a rigid weapon to one character's hand
socket. The reference pose is `idle_aiming` at time `0`.

A fitted weapon is a bake for one rig. It lives in the weapon's folder, named
for the rig it fits:

```text
<weapon folder>/<rig>.gltf
<weapon folder>/<rig>.bin
```

`<rig>` is the character model's folder name. exo_red's rifle bake is
`content/base/models/rifle/exo_red.gltf`, beside the rifle's
first-person viewmodel `model.gltf`. The bake has its own buffer and shares the
folder's textures. Characters on one rig share one bake. A bake never replaces
or edits the viewmodel, its buffer or its textures.

The examples below fit a rifle to exo_red. The weapon's exported glTF
serves as the raw source.

## 1. Bake grip and scale only, to a scratch folder

`prop_to_gltf.py` writes its textures into its output folder, so it would
overwrite the viewmodel's textures if pointed at the weapon folder. Always export
to a scratch folder outside your content, named `<rig>.gltf` so the buffer comes
out as `<rig>.bin`.

First make a grip/scale-only bake from the raw source. Do not supply a rotation
or mount axes at this stage.

```bash
blender --background --python tools/prop_to_gltf.py -- \
  --input content/base/models/rifle/model.gltf \
  --output scratch/rifle/exo_red.gltf \
  --grip 0.025 -0.068 0.082 \
  --scale 0.62
```

`--grip` moves the weapon origin to the hand grip, and `--scale` applies the
uniform size conversion. Scale must be finite and greater than zero; reflected
negative-scale bakes are unsupported in this workflow. Record these values so
the emitted rebake command can regenerate the same file from the raw source.

## 2. Declare the source axes and solve once

The first author must declare the raw-source glTF barrel and up axes once.
They are directions in the **raw-source frame**, before the corrective rotation
is baked. They are not axes copied from the grip/scale-only output.

```bash
bin/postretro-tool solve-weapon-mount content/base/models/exo_red/model.gltf \
  --weapon scratch/rifle/exo_red.gltf \
  --mount-joint hand_r \
  --barrel 0 0 -1 \
  --up 0 1 0 \
  --raw-source content/base/models/rifle/model.gltf \
  --out scratch/rifle/exo_red.gltf \
  --grip 0.025 -0.068 0.082 \
  --scale 0.62
```

`--mount-joint hand_r` selects the joint/socket on the character skeleton. It
is unrelated to `prop_to_gltf.py --socket NAME=NODE`, which writes named prop
attachment-point metadata in the weapon glTF's `extras`; use the latter only
when the prop needs attachments such as `--socket muzzle=BarrelTip`.

When barrel/up axes are declared, the solve output is a trusted declared-axis
path. If they are absent, the tool can inspect the mesh as a geometry assist,
but its output is `UNVERIFIED`; a low-confidence detection remains
`UNVERIFIED` and should not replace an author declaration. The assist command
omits `--mount-axes`, so rebaking it does not turn detected axes into trusted
metadata. A later geometry-only check remains `UNVERIFIED`.

## 3. Run the emitted Blender command

`solve-weapon-mount` prints a Blender command; it does not run Blender itself.
Copy and run that command. It applies the printed `--rotate-euler` and writes
the final asset's mesh-node metadata:

```text
extras.mount = { barrel, up, euler }
```

The persisted barrel/up values remain raw-source-frame declarations. The
persisted Euler is the rotation applied during the final bake.

## 4. Copy the bake into the weapon folder

Copy only the glTF and its buffer. Leave the scratch folder's textures behind.

```bash
cp scratch/rifle/exo_red.gltf scratch/rifle/exo_red.bin \
  content/base/models/rifle/
```

The bake's image references must name textures the weapon folder already holds.

## 5. Check the final bake

Normally, verify the copied asset without re-supplying axes or Euler values:

```bash
bin/postretro-tool solve-weapon-mount content/base/models/exo_red/model.gltf \
  --weapon content/base/models/rifle/exo_red.gltf \
  --mount-joint hand_r \
  --check
```

A declared bake reports `VERIFIED: mount check passed.` A bake checked without
declared axes reports `UNVERIFIED` even when it passes.

The check measures the barrel against the holder model's +Z, the front every
placed model and player body shares, so a holder that does not front +Z fails
`barrel·+Z`. It measures orientation only. Judge scale and grip by eye, in a
third-person or observer view.

For a declared asset, the check composes `extras.mount.euler` into the
raw-source axes to recover the baked-frame directions. That is why the Euler
metadata is required. `--barrel`, `--up`, and `--current-euler` are only
first-author override/fallback inputs for pre-metadata assets; they are not
part of the normal check command. When supplied, CLI axes and Euler override
stale persisted values. A declared solve still computes the full from-raw
Euler; its emitted rebake persists that newly solved Euler and any CLI axis
override.

The sampled socket direction axes must form an orthonormal, positive-determinant
rotation. Fix reflected rig transforms or shear caused by hierarchical
non-uniform scale instead of solving against that invalid frame.

## 6. Declare the mount on the character

A character, player or NPC, names its bakes in `components.mesh.mounts`, one
entry per weapon it may wield (an NPC's entries cover the weapons its attacks
name; see [NPCs carry weapons the same way](#npcs-carry-weapons-the-same-way)). `item` is the weapon's `defineEntity` value, as in
`inventory.loadout`; `attachments` maps sockets to bakes, like
`mesh.attachments`. A weapon built into the character's body takes
`{ item, muzzleSocket: "<body socket>" }` instead: it names no `attachments` and
mounts nothing. An entry names exactly one of the two.

```typescript
import { defineEntity } from "postretro";
import { referencePistolEntity as pistol } from "./reference-pistol";
import { referenceRifleEntity as rifle } from "./reference-rifle";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    mesh: {
      model: "models/exo_red/model.gltf",
      mounts: [
        { item: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
        { item: pistol, attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red.gltf" } },
      ],
    },
    inventory: { loadout: [rifle, pistol] },
  },
});
```

While a weapon is active, its entry's sockets replace `mesh.attachments` on the
same sockets; when the weapon leaves, those sockets return to the base. A weapon
keeps only its first-person `viewmodel`; it has no third-person model of its
own. Load rules and warnings: [Character weapon mounts](scripting-reference.md#character-weapon-mounts).

## Viewmodel muzzle offset

For a projectile weapon, read its viewmodel's rigid `"muzzle"` socket to get a
paste-ready descriptor value:

```bash
bin/postretro-tool solve-weapon-mount --read-muzzle-offset \
  content/base/models/rifle/model.gltf
```

The tool prints `muzzleOffset: [x, y, z]`. Copy the array into the weapon
descriptor. It is the socket's composed rigid rest translation in
mesh-node-local metres, with the same authored scale as the viewmodel vertices.
There is no coordinate conversion or import-time rescaling.

This author-time read is separate from the skinned character mount solver. It
requires a rigid rest socket on the viewmodel and reports an error if `"muzzle"`
is absent or is a skinned joint. The runtime does not look up this socket; only
the copied `muzzleOffset` participates in projectile simulation, so a
third-person mount never moves where a player's shot starts. (An NPC's shot
does start at its mounted muzzle; see
[Where an NPC's shot starts](#where-an-npcs-shot-starts).)

## Pose and character limits

A rigid bake is exact only at the solve pose. A wrist-reorienting animation
(for example, Limitator's `reloading`) needs a skinned weapon rather than a
single rigid bake.

A rigid bake fits only rigs whose hand frames match its solve, so each rig gets
its own bake of each weapon, named `<rig>.gltf` for the character model's folder
name under `content/base/models/`. A rifle held by two characters on different
rigs has two bakes in its folder, such as `content/base/models/rifle/exo_red.gltf`
and `content/base/models/rifle/trooper.gltf`. A folder whose raw source lives
outside your project holds bakes only.

## NPCs carry weapons the same way

An NPC lists its weapons in `components.mesh.mounts`, the table a player uses,
with the same entry shape. The differences are what mounts and when:

- An NPC mounts the entry of **every weapon its attacks name**, all at once. A
  weapon its attacks never name has an entry that does nothing and warns of
  nothing.
- An entry's sockets win over `mesh.attachments` on a socket both name.
- Two attack weapons may share a socket only if they name the **same model**
  there, as the Limitator's two rifles share one bake on `hand_r`. Different
  models on one socket fail the load, naming the character and the socket.
  Entries on different sockets are always fine.
- An NPC with a `mounts` table that lacks an entry for an attack weapon mounts
  nothing for that weapon. An NPC with no table mounts no weapon.

```typescript
export const limitator = defineEntity({
  canonicalName: "limitator",
  components: {
    mesh: {
      model: "models/limitator/model.gltf",
      mounts: [
        { item: burstRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
        { item: longRifle, attachments: { hand_r: "models/ar_4/limitator.gltf" } },
      ],
    },
    // behavior: its attacks name burstRifle's and longRifle's weapons
  },
});
```

## Where an NPC's shot starts

An NPC's shot leaves its fired weapon's **muzzle**, posed on the clip it is
playing, not the top of its head. Every peer draws the same mounted gun; the
shot's start is a host decision.

- **A mounted bake** carries a rigid node named `muzzle` at the barrel tip,
  tagged in the glTF as `extras.socket = "muzzle"`. Add it when you export the
  bake, as for the cyberpunk viewmodels:

  ```bash
  blender --background --python tools/prop_to_gltf.py -- \
    --input content/mymod/models/rifle/model.gltf \
    --output scratch/rifle/trooper.gltf \
    --grip 0.025 -0.068 0.082 --scale 0.62 \
    --socket muzzle=BarrelTip
  ```

  `BarrelTip` is the name of an empty or node you placed at the tip in the
  source; keep it at the tip through any rebake. `solve-weapon-mount` keeps the
  socket and tells you nothing about it: its `--check` proves orientation only.
  **Moving the `muzzle` node moves where the NPC's shots start and which cover
  blocks them**, so re-test an NPC's shots behind low cover after any bake edit.
- **A weapon built into the body** (a cannon fused to an arm) names a body
  socket instead of a bake:

  ```typescript
  mounts: [{ item: armCannon, muzzleSocket: "cannon_tip" }],
  ```

  Its shots leave that posed socket, a skin joint you tag in the model like any
  other socket, and the entry mounts nothing. The engine's source repository has
  an example, `cannon_brute`, in `content/dev/scripts/arm-cannon.ts`; it is not
  part of the SDK bundle.
- **Several `muzzle` sockets.** If one entry mounts several bakes and more than
  one carries a `muzzle`, the shot leaves the first in socket-name order and the
  load warns.

An entry takes exactly one of `attachments` and `muzzleSocket`; both or neither
fails the load.

A pawn ignores `muzzleSocket`: players fire from their first-person placement
plus `muzzleOffset`, whatever their entry says, and the entry mounts nothing.

## Cover near the gun

An NPC does not fire into cover at its gun. Before a shot leaves, the engine
sweeps the shot's radius along two legs: from the NPC's body (its eye) to the
muzzle, then from the muzzle toward the target's aim point for the first 2 m
(less if the target is nearer: the leg then stops about one shot radius short
of the aim point, so a ledge just beside a point-blank target does not hold the
shot). If something blocks either leg, the NPC **holds the shot** rather than
launching it into the lip. A burst checks every shot; an attack with no clip
moment counts once per hold, not once per tick. A held shot counts in
`brain.attacksHeldInActivity`, which a behavior row can read to move on. An
obstacle past 2 m does not hold anything: the shot flies and hits it, which is
how a player ducks behind a column and makes the NPC waste shots. The target's
line of sight (`brain.targetVisible`) is the eye's verdict and is separate:
`targetVisible` can read true while every shot is held.

## When data is missing

Each warning prints once per level load, naming the character and the weapon.
The NPC still works; the missing piece draws or launches as described.

| Problem | What happens |
|---|---|
| A `mounts` table with no entry for an attack weapon | Mounts nothing for it; its shots leave from the eye. |
| An entry socket the character's model lacks | Nothing draws there; warns; that weapon's shots leave from the eye, unless another socket of the entry carries a `muzzle`. |
| An entry model the level did not load | Nothing draws there; warns; that weapon's shots leave from the eye, unless another socket of the entry carries a `muzzle`. |
| No loaded bake carries a `muzzle` socket | Warns; the weapon's shots leave from the eye. |
| Several bakes carry a `muzzle` socket | Warns, naming the one the shots use. |
| A `muzzleSocket` the body model lacks | Warns; shots leave from the eye. |

A character with no `mounts` table at all logs nothing and fires from the eye.
