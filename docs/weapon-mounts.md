# Rigid Weapon Mount Workflow

Use this deterministic workflow to prepare a rigid weapon for a character's
hand socket. The reference pose is `idle_aiming` at time `0`.

## 1. Bake grip and scale only

First make a grip/scale-only bake from the raw source. Do not supply a rotation
or mount axes at this stage.

```bash
blender --background --python tools/prop_to_gltf.py -- \
  --input raw/weapons/ar_4.glb \
  --output content/dev/models/ar_4/model.gltf \
  --grip 0.0 -0.05 0.12 \
  --scale 0.68
```

`--grip` moves the weapon origin to the hand grip, and `--scale` applies the
uniform size conversion. Scale must be finite and greater than zero; reflected
negative-scale bakes are unsupported in this workflow. This is the final weapon
path; record these values so the emitted rebake command can regenerate that same
file from the raw source.

## 2. Declare the source axes and solve once

The first author must declare the raw-source glTF barrel and up axes once.
They are directions in the **raw-source frame**, before the corrective rotation
is baked. They are not axes copied from the grip/scale-only output.

```bash
cargo run -p xtask -- solve-weapon-mount content/dev/models/limitator/model.gltf \
  --weapon content/dev/models/ar_4/model.gltf \
  --mount-joint hand_r \
  --barrel 0 1 0 \
  --up 0 0 1 \
  --raw-source raw/weapons/ar_4.glb \
  --out content/dev/models/ar_4/model.gltf \
  --grip 0.0 -0.05 0.12 \
  --scale 0.68
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

## 4. Check the final bake

Normally, verify the final asset without re-supplying axes or Euler values:

```bash
cargo run -p xtask -- solve-weapon-mount content/dev/models/limitator/model.gltf \
  --weapon content/dev/models/ar_4/model.gltf \
  --mount-joint hand_r \
  --check
```

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

## Viewmodel muzzle offset

For a projectile weapon, read its viewmodel's rigid `"muzzle"` socket to get a
paste-ready descriptor value:

```bash
cargo run -p xtask -- solve-weapon-mount --read-muzzle-offset \
  content/dev/models/ar_4/model.gltf
```

The tool prints `muzzleOffset: [x, y, z]`. Copy the array into the weapon
descriptor. It is the socket's raw glTF rest translation in model-local metres:
the same mesh-node-local frame and authored scale as the viewmodel vertices.
There is no Blender up-axis conversion or import-time rescaling.

This author-time read is separate from the skinned character mount solver. It
requires a rigid rest socket on the viewmodel and reports an error if `"muzzle"`
is absent or is a skinned joint. The runtime does not look up this socket; only
the copied `muzzleOffset` participates in projectile simulation.

## Pose and character limits

A rigid bake is exact only at the solve pose. A wrist-reorienting animation
(for example, Limitator's `reloading`) needs a skinned weapon rather than a
single rigid bake. The default is one baked weapon file per weapon. If
characters use different hand frames, separate baked weapon files per
character are the escape hatch.
