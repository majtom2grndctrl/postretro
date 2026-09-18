# Slice 2 pre-directory cold baseline

**Status: captured and verified (2026-09-18).**

This is the pre-Task-4 preservation baseline for the inert cluster-directory
slice. It intentionally predates section 49 behavior. Task 7 compares every
pre-existing section body and container-entry version against a directory bake
in this relative order; absolute offsets are excluded because adding one table
entry necessarily moves them.

## Inputs and recipe

`gate-heavily-lit.map` is the existing compact, purpose-built heavily lit SH
gate fixture. The compiler fixture documentation prefers it over
`campaign-test` for exact SH bake coverage because it preserves the relevant
light-count stress at a fraction of the bake cost.

| Control | Value |
| --- | --- |
| Source revision | `25ebc1e7bed14bf03583d5b9dd77b881edca171f` (`feature/sh-probe-streaming` at baseline worktree creation) |
| Baseline branch | `codex/sh-probe-streaming-s2-baseline` |
| Source map | `content/dev/maps/gate-heavily-lit.map` |
| Source map SHA-256 | `96c107e4b3cb66e477b16e9376a305d44dffa544e70427859274e51784677faa` |
| Cache mode | cold, `--no-cache` |
| Compiler permits | 1 (`-j 1`) |
| Irradiance encoding | uncompressed (`--uncompressed-irradiance`) |
| Forced SH hierarchy scale | 1 (`--sh-density-force-scale 1`) |
| Shared Cargo target | `/Users/dhiester/Projects/Personal/postretro/target` |
| Retained baseline | `/private/tmp/postretro-sh-cluster-baseline/gate-heavily-lit.pre-directory.uncompressed.prl` |

Run from the repository root in the baseline worktree:

```sh
mkdir -p /private/tmp/postretro-sh-cluster-baseline
CARGO_TARGET_DIR=/Users/dhiester/Projects/Personal/postretro/target cargo run -p postretro-level-compiler -- content/dev/maps/gate-heavily-lit.map -o /private/tmp/postretro-sh-cluster-baseline/gate-heavily-lit.pre-directory.uncompressed.prl --no-cache --no-tui -j 1 --uncompressed-irradiance --sh-density-force-scale 1
```

The bake completed successfully in 50.52 seconds. It emitted 18 expected
fixture warnings: 17 missing-style defaults and one shadowmask four-channel
assignment warning. No cache or directory section participated.

## Container identity

| Property | Value |
| --- | --- |
| Container version | 4 |
| Section count | 22 |
| Whole PRL bytes | 30,480,375 |
| Whole PRL SHA-256 | `757dc6ae72e0c3362579b7ef5871c37f6bcbd82889bf1ce98fb6d3d1ea016637` |

## Existing entries in relative order

Hashes cover each section body only.

| Relative index | Section id | Entry version | Body bytes | Body SHA-256 |
| ---: | ---: | ---: | ---: | --- |
| 0 | 17 | 1 | 6,700 | `83e6be76da495811d2d3653e03fe8363825c5b0556f2e9521598124d1ee0d01d` |
| 1 | 16 | 1 | 29 | `d2a441d035185bbd3d42c6b1c22c56eb906ffb6a175c329569189726a5debf86` |
| 2 | 32 | 1 | 36 | `ab02ff6cee0e3ecd52759c05f95c22613a4b51ec59f0a177850d46e98f5e90c5` |
| 3 | 38 | 1 | 2,004 | `4f4fa8649f9553d026b8996eae7f4317a3e8b7c4193521baf712f9fe83b065f6` |
| 4 | 39 | 1 | 1,104 | `3809e7ca1b1d2f25f3a7e9186fbdef29497b1595ba4a8b3d6ef125f6c20e28b6` |
| 5 | 15 | 1 | 3,592 | `40aa4f61bc674b9151d93291fbc388abd5ff0006ecb222d9fb79626b5c7e1b58` |
| 6 | 23 | 1 | 1,168 | `252fb037ec231abff2f6ac34f66abbba90033e738e2e15d520b3eacaffc2d553` |
| 7 | 19 | 1 | 4,840 | `1a131e5eea211f66ffd451206459b5cb6c07f0ef5d4a204b1193989a58498a50` |
| 8 | 18 | 1 | 1,249 | `47d81c39bf5ca826800eb6ea4b9eb543de4a044aad6247ae052e682353b9f433` |
| 9 | 21 | 1 | 288 | `27ca7b4827894eeed8fb49cd0e970f6ec63932d2cc7ef17dce4d9b5b41b042d8` |
| 10 | 34 | 1 | 1,021,404 | `15c876056b80bcefbf908e217e06210eb801f760d03970c45d856816b4e6a6b8` |
| 11 | 22 | 1 | 17,825,840 | `2351932888b7f0869a70ff27d77e4d774007984df08fdab545c0862f47292f75` |
| 12 | 35 | 1 | 985,612 | `dfcea88ed1882dd72602fdf3ad978d5f039c1f1fbe729a0223b1e3bb32a4f58c` |
| 13 | 40 | 1 | 68 | `35ad69d8d975703d527e8c233b5987f478965658c3e84d44c70e4415532bd808` |
| 14 | 41 | 1 | 2,199,574 | `3048f6030c4f553582c4492e62ea5d05c772b8f070334119eac1e94159fe99b5` |
| 15 | 42 | 1 | 8,388,640 | `ff34ddf884b4474bc54da0d5dfead3f13f05f28627fe52753d404b3d69129ea2` |
| 16 | 47 | 1 | 35,749 | `4589cca8068fb1ae8030deb874564909b853256e980129255faadcd955ad9159` |
| 17 | 29 | 1 | 52 | `012dc91237969917f73d94d7dbdac3c6268d2010dcacf4146cc4cabd9b55bf95` |
| 18 | 30 | 1 | 12 | `b5a054fc17bb0ce57932c9d154bd87a4d7a25699765a3f738ec7956a60202c04` |
| 19 | 36 | 1 | 578 | `666908e8ce9df04c99417f98ea96f8ce14a4a18ff305493b12fe0a738f752946` |
| 20 | 37 | 1 | 232 | `c8facde8771a29506c393df9605e58f9d3d180ac0a466c29e89b2a7692e636b4` |
| 21 | 46 | 1 | 1,112 | `8c9ab13be25f8d861e8dce07bdbba12ad46f9f7b2fc8f791bba6ef555e84fcc1` |

## Verification and cleanup

A temporary read-only Ruby inspection command decoded the 8-byte PRL header
and each 22-byte table entry directly, checked the first payload offset, checked
every body range, recomputed every body SHA-256, and proved that the final entry
end equals the 30,480,375-byte file length. `shasum -a 256` independently
produced the source-map and whole-file hashes above. The artifact remains at the
recorded `/private/tmp` path for Task 7; it is not tracked by Git.

After Task 7 has compared every pre-existing entry and recorded the result,
remove `/private/tmp/postretro-sh-cluster-baseline/`. If the temporary artifact
is lost before then, reproduce it only from the exact revision and command
above and verify that its whole-file hash matches before using it as evidence.
