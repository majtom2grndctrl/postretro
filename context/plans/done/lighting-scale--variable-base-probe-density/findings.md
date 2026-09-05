# Variable base-density probe storage — measurement findings

Measurement date: 2026-09-04.  This is a measurement record, not a visual-signoff record.

## Method and limits

Both development maps were compiled successfully at 1.0 m probe spacing with the
feature branch at `181fc7605`, first as a uniform-L0 stored baseline and then as
the classified result.  The compiler was invoked through
`cargo run -p postretro-level-compiler -- <map> -o <temporary-output> --no-tui
--sh-probe-spacing 1.0 --cache-dir /private/tmp/postretro-density-cache`, with
the default BC6H storage format and again with `--sh-volume-format rgba16f`.
The cache was intentionally outside the worktree.  These are warm development
bakes; the compiler labels their indirect lighting as approximate.  The source
maps produced their existing diagnostics (campaign: 61, kinematic: 8), but both
commands exited successfully.

`Dense` below is the unmodified current-main runtime atlas contract derived from
the emitted grid: one Rgba16Float tile is 288 bytes and the dense representation
has one tile per probe-grid cell, for each of the total and direct atlases.  It
is the relevant composed-atlas VRAM comparison.  An isolated pre-feature/v9 bake
was not produced, so this record does **not** present format-section file bytes
from that incompatible wire format as a before value.

## AC12 — composed total + direct atlas VRAM

The total and direct composed atlases have the same tile geometry in each row;
the bytes below are their combined runtime allocation, not compressed PRL bytes.

| Map | storage form | tiles per atlas | total + direct bytes | MiB | reduction vs. dense |
| --- | --- | ---: | ---: | ---: | ---: |
| campaign-test | dense current-main contract | 194,028 | 111,760,128 | 106.58 | 1.000x |
| campaign-test | uniform stored L0 | 57,128 | 32,905,728 | 31.38 | 3.396x |
| campaign-test | classified stored | 28,268 | 16,282,368 | 15.53 | 6.864x |
| kinematic-platform | dense current-main contract | 133,055 | 76,639,680 | 73.09 | 1.000x |
| kinematic-platform | uniform stored L0 | 118,231 | 68,101,056 | 64.95 | 1.125x |
| kinematic-platform | classified stored | 34,954 | 20,133,504 | 19.20 | 3.807x |

Classification further reduces the uniform stored allocation by 2.021x on
campaign-test and 3.383x on kinematic-platform.  This supports the composed
runtime allocation part of AC12 on two maps.  It does not substitute a physical
pre-feature PRL byte comparison.

## AC13 — id34 / id35 serialized section bytes

These are exact compiler-reported section bytes.  `Before` means the same
post-format uniform-L0 bake, rather than a pre-feature v9 file; that distinction
is necessary because the new section schema and header version intentionally
change the serialized representation.

| Map / format | id34 total: uniform L0 -> classified | factor | id35 direct: uniform L0 -> classified | factor |
| --- | ---: | ---: | ---: | ---: |
| campaign-test / BC6H (tag 1) | 3,621,236 -> 2,577,524 B | 1.405x | 2,067,916 -> 1,024,204 B | 2.019x |
| campaign-test / RGBA16F raw (tag 0) | 18,073,076 -> 9,730,292 B | 1.857x | 16,519,756 -> 8,176,972 B | 2.020x |
| kinematic-platform / BC6H (tag 1) | 5,324,696 -> 2,327,976 B | 2.287x | 4,260,172 -> 1,263,452 B | 3.372x |

The raw campaign bake isolates classification/geometry from BC6H compression.
Kinematic raw bytes and a true pre-feature file-byte baseline were not collected;
both remain follow-up work if serialized before/after accounting is required.

## AC14 — class histogram and delta-pin attribution

| Map | L0 bricks | L1 bricks | L2 bricks | stored tiles | delta-pin entries id27/id41/id45 | protection pins |
| --- | ---: | ---: | ---: | ---: | ---: |
| campaign-test | 2,554 (77.253%) | 390 (11.797%) | 362 (10.950%) | 28,268 | 48 / 12 / 18 | 0 |
| kinematic-platform | 888 (37.852%) | 711 (30.307%) | 747 (31.841%) | 34,954 | 0 / 0 / 0 | 0 |

Campaign has 78 delta-pin attribution entries (2.359% of 3,306 classified
bricks).  This is an attribution count, not a unique-brick fraction: the format
permits a brick to appear in more than one delta bucket.  The campaign pin cost
is therefore small by count, but this data alone cannot distinguish a necessary
detail-preservation cost from a threshold issue.  Kinematic-platform exercises
the classification without any delta/protection pin contribution.

## AC15 — CPU sampler-cost diagnostic and GPU timing gate

I parsed each classified id34 volume and evaluated the shader's whole-cell
resolution and per-corner tap contract once for every interior base-grid sample
cell (one atlas/direction; outer depth/backface blending is deliberately not
counted).  A whole-cell L1 executes eight taps; corner L1 paths execute the
shader's local 0/3 vs. inner tap count.  This measures issued lookup work, not
GPU elapsed time.

| Map | sampled cells | whole-cell L1 | whole-cell L2 | mean distinct tiles | mean tap instructions | maximum distinct tiles | maximum taps |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| campaign-test | 181,478 | 8,655 (4.769%) | 6,424 (3.540%) | 2.1350 | 3.0528 | 8 | 32 |
| kinematic-platform | 123,904 | 19,197 (15.493%) | 20,169 (16.278%) | 5.3751 | 8.8621 | 8 | 32 |

The distinct-tile ceiling was 8 for every sampled cell, as required by the
sampling contract.  Non-zero diagnostic distributions were:

| Map | distinct-tile buckets (cells) | tap-instruction buckets (cells) |
| --- | --- | --- |
| campaign-test | 1:7,534; 2:6,919; 3:301; 4:10,843; 5:957; 6:1,155; 7:256; 8:38,537 | 1:6,468; 2:1,282; 3:164; 4:10,300; 5:520; 6:1,132; 7:110; 8:31,642; 9:776; 10:1,658; 11:396; 12:1,588; 13:2,226; 14:198; 15:17; 16:2,571; 18:2,198; 20:530; 24:2,184; 32:542 |
| kinematic-platform | 1:21,217; 2:16,406; 3:103; 4:12,703; 5:6,992; 6:2,362; 7:1,259; 8:62,862 | 1:20,171; 2:312; 3:4; 4:10,176; 5:224; 6:331; 7:34; 8:58,263; 9:956; 10:3,608; 11:1,048; 12:3,280; 13:5,294; 14:537; 16:6,276; 18:5,290; 20:1,411; 24:5,341; 32:1,348 |

The zero-work cells omitted from the compact distribution table were 114,976
(63.355%) for campaign-test; kinematic-platform has no zero-work cells.

GPU timing remains **not evaluated**.  A bounded default campaign engine launch
was attempted with `POSTRETRO_GPU_TIMING=1` and dev-tools enabled.  The binary
reached `[Engine] Postretro starting`, then the macOS window service reported a
connection-invalid error and it never emitted renderer initialization, adapter
name, or a timing sample.  It was stopped after the bounded wait.  Consequently
there is no named Turing adapter result and no laptop shared-memory iGPU result;
neither adapter can be inferred from this host.

## AC16 and live honesty gates

Manual visual sign-off is **not evaluated**, not a parity pass.  This environment
did not provide a usable interactive engine window after the default launch, so
the following could not be observed:

| Gate | result | blocker |
| --- | --- | --- |
| default campaign classified boot / AC2 | not evaluated | renderer never initialized after window-service failure |
| forced L1 and forced L2 on campaign, kinematic, and stress-warren-mini / AC3 | not evaluated | no usable interactive renderer; forced cases not launched |
| dev-tools density marker / AC10 | not evaluated | no interactive frame/overlay |
| degenerate volumes / AC11 | not evaluated | no usable interactive renderer |
| per-frame summary noise gate | not evaluated | no rendered frames/log stream |
| AC16 visual sweep (default plus forced L1/L2) | not evaluated | no manual viewport available |

No live visual observation is being represented as pass/fail parity, and no GPU
claim is being inferred from the CPU diagnostic.

## Recommendation

Keep the storage and CPU evidence, but do not use this record to grant a
fidelity/performance ship sign-off.  Classification materially lowers composed
atlas allocation on both maps and preserves the eight-distinct-tile bound, while
campaign's 78 delta-pin attributions are modest enough not to independently
justify reopening the threshold questions.  Before accepting the default as
shippable, rerun the visual sweep and GPU timestamp capture on a functioning
interactive target (specifically a named Turing-class adapter and a laptop
shared-memory iGPU), then collect an isolated pre-feature bake if exact legacy
serialized-byte comparison is still a release criterion.
