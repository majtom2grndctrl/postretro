# Cell-rate spike: Metal System Trace kit (5300M)

Times the fluid reflection cell-rate arms on a Mac and checks their capture-image honesty gates. It is adapted from `measurements/reflection-blend-cost-spike/` (branch `claude/probe-streaming-brief-364a5e`). Results go to `../results-5300m.{md,json}`.

## Setup

From the repo root, on `claude/cell-rate-spike`:

```
cargo build --release -p postretro --features capture,dev-tools
cargo build --release -p postretro-level-compiler --bin prl-build
cargo build --release -p postretro-script-compiler --bin scripts-build   # prl-build needs it for the probes map
target/release/prl-build content/dev/maps/e24-environment-test.map --no-cache
target/release/prl-build content/dev/maps/reflection-probes-test.map --no-cache
```

The engine binary must be named `postretro`, because `gpu_time.py` keeps only trace rows from a process of that name. To time a frozen copy, put it in its own directory and pass `CELL_BIN=<dir>/postretro`.

## Timing

```
python3 measurements/cell-rate-spike/metal/run.py --list          # arms
python3 measurements/cell-rate-spike/metal/run.py                 # every arm, 2 interleaved rounds
python3 measurements/cell-rate-spike/metal/run.py --only a,b      # chosen arms
python3 measurements/cell-rate-spike/metal/run.py --summarize-only  # rebuild results from every runs/*.run.json
CELL_IMPL=1660 CELL_BIN=<1660 build>/postretro python3 measurements/cell-rate-spike/metal/run.py  # the 1660 branch's arms
```

**Arms:** config × mode × ratio.
- **Configs:** near, probes (`test`), far, low, mid and tilt poses at 4K or 1080p. The `test-540/360/270` configs were tried and discarded (low clock).
- **Modes:** `frag`, `cells`, `cull` and `free` (light mask without the specular bit).
- **Ratio:** live, or `r1` (forced to 1).

**Each run:**
- One headless `--capture` under `xctrace record --launch` (Metal System Trace), 240 warmup and 240 sampled frames.
- The export of `metal-gpu-intervals` is reduced with the complete-frame method.
- Cost = Fluid Surface Pass + Fluid Reflection Cell Pass per frame.

**The batch:**
- Holds caffeinate and re-declares user activity.
- Waits 15 s before each launch.
- Polls the ioreg clock and temperature.
- Refuses to run beside another engine, `cargo`, `rustc` or `prl-build`.
- Marks a run invalid on a screen lock, the screen saver, or a trace that dropped rows carrying time.
- Deletes each trace and its `$TMPDIR` `instruments*.ktrace` once reduced. Gzipped exports stay in `raw/`, which is gitignored.

Keep the machine on mains power with the screen awake, and run no GPU-heavy apps during a batch.

## Gates

```
MAIN_BIN=<origin/main build>/postretro python3 measurements/cell-rate-spike/metal/gates.py [--only parity,parity-t,ratio1,main,free,smooth,stale,stats] [--configs near-4k,...]
```

The gates take untraced captures and compare them per channel after decoding the PNGs through `sips`.

| Gate | Compares |
|---|---|
| parity, ratio1 | frag against cells against cull, at animation time 0 |
| parity-t | frag against cells after 4+4, 30+30 and 120+120 frames |
| main | the spike with levers off against main, which must be bit-identical |
| free | the reflection-free variant with every lever on against main |
| smooth | smooth style with cells on against off |
| stale | a capture that starts from another pose against a fresh one |
| stats | atlas size, cells dispatched and evaluated, visible reflecting fragments |

Results go to `gates.json`; PNGs to `gate-runs/` (gitignored).
