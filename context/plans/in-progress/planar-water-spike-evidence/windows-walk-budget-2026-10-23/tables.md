# Walk budget live results

Release + dev-tools/capture, ordinary streaming, Vulkan, indirect validation off, VSync off. Entries are run 1 / run 2. CPU entries are five tail 120-frame window means; GPU entries are three tail 120-readback window means (logs round to 0.01 ms). CPU and GPU windows do not align. `mirror_walk` times only the mirror portal walk; the fallback per-cell cull is not in it but is inside `visibility`. `visibility` also contains the main walk (`portal_walk`).

| Configuration | Frame, ms | Work, ms | Visibility, ms | Main walk, ms | Mirror walk, ms | Submit, ms | Reflection GPU, ms | planar_forward, ms | Mirror path | Considered | Drawable cells | Mirror-only |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|
| ssr-high | 4.745 / 4.717 | 4.628 / 4.601 | 2.925 / 2.914 | 2.913 / 2.902 | absent / absent | 0.705 / 0.696 | 0.943 / 0.950 | absent / absent | - / - | - / - | - / - | - / - |
| ssr-very-high | 5.071 / 5.079 | 4.800 / 4.797 | 2.962 / 2.965 | 2.949 / 2.953 | absent / absent | 0.841 / 0.839 | 1.747 / 1.750 | absent / absent | - / - | - / - | - / - | - / - |
| half-unlimited | 7.719 / 7.635 | 7.598 / 7.514 | 5.746 / 5.663 | 3.023 / 2.946 | 2.692 / 2.686 | 0.809 / 0.811 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback / StepLimitFallback | 20000 / 20000 | 126 / 126 | 0 / 0 |
| half-unlimited-w2000 | 5.200 / 5.259 | 5.080 / 5.135 | 3.261 / 3.259 | 2.948 / 2.946 | 0.284 / 0.282 | 0.806 / 0.834 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback / StepLimitFallback | 2000 / 2000 | 126 / 126 | 0 / 0 |
| half-unlimited-w500 | 5.047 / 4.956 | 4.928 / 4.839 | 3.120 / 3.022 | 3.019 / 2.920 | 0.071 / 0.072 | 0.805 / 0.803 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback / StepLimitFallback | 500 / 500 | 126 / 126 | 0 / 0 |
| half-unlimited-w0 | 4.969 / 4.962 | 4.848 / 4.842 | 3.016 / 3.011 | 2.986 / 2.982 | 0.000 / 0.000 | 0.810 / 0.818 | 0.210 / 0.210 | 0.140 / 0.140 | StepLimitFallback / StepLimitFallback | 0 / 0 | 126 / 126 | 0 / 0 |
| half-d15 | 4.953 / 4.961 | 4.834 / 4.841 | 3.043 / 3.039 | 2.957 / 2.952 | 0.060 / 0.061 | 0.794 / 0.800 | 0.090 / 0.090 | 0.030 / 0.030 | Seeded / Seeded | 354 / 354 | 11 / 11 | 0 / 0 |

Window probe per run (2 s samples): overlay = NVIDIA GeForce Overlay above the engine (noted confounder); user = user window above (excludes); foreground = engine had focus.

| Run | Samples | Overlay | User above | Minimized | Foreground | Excluded |
|---|---:|---:|---:|---:|---:|---|
| ssr-high-run1 | 10 | 7 | 0 | 0 | 0 | False |
| ssr-very-high-run1 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-run1 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-w2000-run1 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-w500-run1 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-w0-run1 | 10 | 7 | 0 | 0 | 0 | False |
| half-d15-run1 | 10 | 7 | 0 | 0 | 0 | False |
| half-d15-run2 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-w0-run2 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-w500-run2 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-w2000-run2 | 10 | 7 | 0 | 0 | 0 | False |
| half-unlimited-run2 | 10 | 7 | 0 | 0 | 0 | False |
| ssr-very-high-run2 | 10 | 7 | 0 | 0 | 0 | False |
| ssr-high-run2 | 10 | 7 | 0 | 0 | 0 | False |
