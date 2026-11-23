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
`content/dev/models/cyberpunk_weapons/rifle/exo_red.gltf`, beside the rifle's
first-person viewmodel `model.gltf`. The bake has its own buffer and shares the
folder's textures. Characters on one rig share one bake. A bake never replaces
or edits the viewmodel, its buffer or its textures.

The examples below fit the dev rifle to exo_red. The weapon's exported glTF
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
  --input content/dev/models/cyberpunk_weapons/rifle/model.gltf \
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
bin/postretro-tool solve-weapon-mount content/dev/models/exo_red/model.gltf \
  --weapon scratch/rifle/exo_red.gltf \
  --mount-joint hand_r \
  --barrel 0 0 -1 \
  --up 0 1 0 \
  --raw-source content/dev/models/cyberpunk_weapons/rifle/model.gltf \
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
  content/dev/models/cyberpunk_weapons/rifle/
```

The bake's image references must name textures the weapon folder already holds.

## 5. Check the final bake

Normally, verify the copied asset without re-supplying axes or Euler values:

```bash
bin/postretro-tool solve-weapon-mount content/dev/models/exo_red/model.gltf \
  --weapon content/dev/models/cyberpunk_weapons/rifle/exo_red.gltf \
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

A character names its bakes in `components.mesh.weaponMounts`, one entry per
weapon it may wield. `weapon` is the weapon's `defineEntity` value, as in
`inventory.loadout`; `attachments` maps sockets to bakes, like
`mesh.attachments`.

```typescript
import { defineEntity } from "postretro";
import { referencePistolEntity as pistol } from "./reference-pistol";
import { referenceRifleEntity as rifle } from "./reference-rifle";

export const playerEntity = defineEntity({
  canonicalName: "player",
  components: {
    mesh: {
      model: "models/exo_red/model.gltf",
      weaponMounts: [
        { weapon: rifle, attachments: { hand_r: "models/cyberpunk_weapons/rifle/exo_red.gltf" } },
        { weapon: pistol, attachments: { hand_r: "models/cyberpunk_weapons/pistol/exo_red.gltf" } },
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
  content/dev/models/cyberpunk_weapons/rifle/model.gltf
```

The tool prints `muzzleOffset: [x, y, z]`. Copy the array into the weapon
descriptor. It is the socket's composed rigid rest translation in
mesh-node-local metres, with the same authored scale as the viewmodel vertices.
There is no coordinate conversion or import-time rescaling.

This author-time read is separate from the skinned character mount solver. It
requires a rigid rest socket on the viewmodel and reports an error if `"muzzle"`
is absent or is a skinned joint. The runtime does not look up this socket; only
the copied `muzzleOffset` participates in projectile simulation, so a
third-person mount never moves where a shot starts.

## Pose and character limits

A rigid bake is exact only at the solve pose. A wrist-reorienting animation
(for example, Limitator's `reloading`) needs a skinned weapon rather than a
single rigid bake.

A rigid bake fits only rigs whose hand frames match its solve, so each rig gets
its own bake of each weapon. The rifle the Limitator holds is
`content/dev/models/ar_4/limitator.gltf`; the trooper and warrior rigs each have
their own bake in that folder. A folder whose raw source lives outside your
project, like `ar_4/`, holds bakes only.

Weapons NPCs carry still mount through `mesh.attachments`, not `weaponMounts`.
