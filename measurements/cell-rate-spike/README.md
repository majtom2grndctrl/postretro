# Cell-rate spike measurements

Data for `context/plans/drafts/reflection-probe-fluid-pass-performance--cell-rate-spike/findings.md`. Two machines measured two independent implementations; their files sit side by side here.

| Path | What |
|---|---|
| `results-5300m.{md,json}` | Radeon Pro 5300M, Metal System Trace: this branch's implementation (`3f4cabed5`) |
| `results-5300m-impl1660.{md,json}` | The same kit timing `claude/cell-rate-spike-1660`'s binary (`cab5197a5`) on the 5300M |
| `metal/` | The 5300M kit (`run.py`, `gpu_time.py`, `gates.py`), its per-run JSON (`runs/`, `runs-impl1660/`), and its capture-image gates (`gates.json`, `gate-runs/`). See `metal/README.md`. |
| `results-1660.{md,json}` | GTX 1660 Super, Vulkan engine timestamps: the 1660 branch's implementation |
| `gates-1660.{md,json}`, `gate-split-cells.json` | The 1660 run's gates |
| `runs/` | The 1660 run's per-run JSON (`ts-1660-*`, `maskfree-1660-*`) and `count-stats.json` |
| `1660/` | The 1660 kit, copied unchanged from `claude/cell-rate-spike-1660` at `f63f94cd4`. Its paths assume it runs from this directory. |
