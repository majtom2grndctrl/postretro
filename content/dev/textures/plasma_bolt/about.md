`plasma_bolt_00.png` … `plasma_bolt_59.png` — 60-frame flipbook for the projectile
weapon enhancements reference plasma round (a blue energy burst, 32×32 RGBA).

Collection convention: a bare top-level collection name (`plasma_bolt`) with
numeric-suffix frames `plasma_bolt_NN.png`, loaded by
`postretro_render_cpu::fx::smoke::load_collection_frames` (a subpath collection
like `sprites/plasma_bolt` would NOT resolve — the loader's frame-name prefix is
the collection string itself). Referenced by the reference plasma-bolt weapon in
`context/plans/drafts/projectile-weapon-enhancements`.

Source: user-supplied "Effects_Pack_14" upload, folder `1` (frames `1.png`…`60.png`
remapped to `plasma_bolt_00`…`_59` in numeric order).

**License: UNVERIFIED.** The uploaded pack bundled no LICENSE/readme. Confirm the
pack's license and attribution before this asset ships (cf. the sibling
`content/dev/textures/projectiles/LICENSE.txt` convention).
