# B3 GPU bandwidth validation

Validated at integration commit `a9f4f31a` on 2026-08-10.

## Access pattern

Source review passes for all three compose shaders.

- One `8×8×1` workgroup owns one `4×4×4` affinity brick.
- L0 keeps direct compact-payload reads. It does not fill shared memory.
- L1 loads only kept corner tiles into workgroup memory: at most eight tiles per CSR entry.
- L2 loads one representative mean tile per CSR entry.
- All 64 dense output probes consume that shared lattice before the next entry.
- Probe tile writes use the brick-to-scattered-atlas mapping; reconstruction stays intra-brick.

This is the AC 9 gate. The renderer exposes no byte-read counter, so GPU time is corroboration only.

## `combat-demo` bake

Commands:

```text
/private/tmp/postretro-lighting-coarsening-b3-target/debug/prl-build content/dev/maps/combat-demo.map -o /private/tmp/postretro-lighting-coarsening-b3-results/combat-demo-baseline.prl
env RUST_LOG=info /private/tmp/postretro-lighting-coarsening-b3-target/debug/prl-build content/dev/maps/combat-demo.map --sh-coarsen -o /private/tmp/postretro-lighting-coarsening-b3-results/combat-demo-coarsened.prl
```

| Artifact | Whole PRL | id-41 raw payload | CSR entries |
|---|---:|---:|---:|
| Baseline | 12,660,208 B | 675,648 B | 48 |
| `--sh-coarsen` | 12,507,568 B | 523,008 B | 48 |

The coarsened bake logged `[sh-coarsen] classified coarsening levels for 1 delta section(s)`. `combat-demo` has no id-27 or id-45 delta payload; its id-41 direct delta is the known coarsening case.

## GPU timing and readback

Attempted baseline launch:

```text
env CARGO_TARGET_DIR=/private/tmp/postretro-lighting-coarsening-b3-target RUST_LOG=info POSTRETRO_GPU_TIMING=1 cargo run -p xtask -- run /private/tmp/postretro-lighting-coarsening-b3-results/combat-demo-baseline.prl
```

The engine compiled and started, but did not reach renderer initialization. macOS emitted:

```text
Failure on line 688 in function id scheduleApplicationNotification(...): noErr == _LSModifyNotification(...)
Connection Invalid error for service com.apple.hiservices-xpcservice.
Error received in message reply handler: Connection invalid
```

No `GPU timing enabled`, timestamp-feature warning, or `sh_compose` / `direct_sh_compose` / `animated_direct_sh_compose` window was logged. The B3-owned engine was stopped. The same LaunchServices blocker makes a coarsened timing run and the optional large-map bake non-actionable here.

Raw per-texel readback comparison is unavailable without a code change: the existing dev-tools readback is UI-gated and publicly returns only a per-probe average RGB decode. B3 does not add observability solely for this check.

At report time, no compiler or engine process tied to the B3 worktree or result paths remained alive.
