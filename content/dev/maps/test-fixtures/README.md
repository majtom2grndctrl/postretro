# SH streaming hinted doorway fixture

`sh-streaming-hinted-door.prl` is the checked-in loader fixture for
`content/dev/maps/sh-streaming-hinted-door.map`. It is intentionally small so
the PostRetro controller test can load real v2 id-49/id-50 metadata without a
recursive Cargo invocation.

Regenerate it after changing the source map or compiler output with a built
`prl-build` binary:

```sh
target/debug/prl-build content/dev/maps/sh-streaming-hinted-door.map \
  --sh-probe-spacing 4 \
  -o content/dev/maps/test-fixtures/sh-streaming-hinted-door.prl --no-cache
```

For the deterministic-bake check, make two `--no-cache` outputs in temporary
paths and compare them with `cmp -s`; they must also match this fixture byte
for byte. The fixture preserves a near-side resident pin, a far-side priority
of 3, and a seam whose far endpoint is unpinned so the closed-door controller
test can assert `SeamWarm` exactly.
