# SH streaming hinted doorway fixture

`sh-streaming-hinted-door.prl` is the checked-in loader fixture for
`content/dev/maps/sh-streaming-hinted-door.map`. It is intentionally small so
the PostRetro controller test can load real id-49 v3 and id-50 v2 metadata
without a recursive Cargo invocation.

Regenerate it after changing the source map or compiler output with a built
`prl-build` binary:

```sh
target/debug/prl-build content/dev/maps/sh-streaming-hinted-door.map \
  --sh-probe-spacing 4 --whole-load-sh \
  -o content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl --no-cache
```

`--whole-load-sh` keeps the bodies of ids 27, 34, 35, 41 and 45: a compiler
test decodes those sections whole from this fixture, and the level then also
loads under `POSTRETRO_SH_STREAMING=off`.

For the deterministic-bake check, make two `--no-cache` outputs in temporary
paths with the same flags as the command above, `--whole-load-sh` included, and compare them with `cmp -s`; they must also match this fixture byte
for byte. The fixture preserves a near-side resident pin, a far-side priority
of 3, and a seam whose far endpoint is unpinned so the closed-door controller
test can assert `SeamWarm` exactly.
