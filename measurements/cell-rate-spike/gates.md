# Cell-rate spike: capture-image gates

Spike `65ca0fbb3946014409dfbc4c8ee899956e828584` vs origin/main (`postretro-main-ab.exe`, with the local `SPIKE_AB_SMOOTH` hook). Every capture renders warmup 4 + 4 frames. Differences are max channel |a - b| on the 8-bit PNG.

| view | comparison | max | px > 0 | px > 2 | of px |
|---|---|---|---|---|---|
| near-4k | cell_vs_frag_live | 9 | 22184 | 840 | 8294400 |
| near-4k | cell_vs_frag_r1 | 9 | 22184 | 840 | 8294400 |
| near-4k | cellcull_vs_cell_live | 0 | 0 | 0 | 8294400 |
| near-4k | cell2_vs_cell_live | 1 | 224186 | 0 | 8294400 |
| near-4k | cell2_vs_frag_live | 9 | 216347 | 840 | 8294400 |
| near-4k | frag2_vs_frag_live | 1 | 224076 | 0 | 8294400 |
| near-4k | spike_frag_vs_main_pixel | 0 | 0 | 0 | 8294400 |
| near-4k | spike_smooth_vs_main_smooth | 0 | 0 | 0 | 8294400 |
| near-4k | spike_smooth_cell_lever_vs_main_smooth | 0 | 0 | 0 | 8294400 |
| near-4k | spike_free_cell_lever_vs_main_free | 0 | 0 | 0 | 8294400 |
| near-4k | smooth differs from pixel (sanity) | True | | | |
| near-1080p | cell_vs_frag_live | 9 | 5109 | 164 | 2073600 |
| near-1080p | cell_vs_frag_r1 | 9 | 5109 | 164 | 2073600 |
| near-1080p | cellcull_vs_cell_live | 0 | 0 | 0 | 2073600 |
| near-1080p | cell2_vs_cell_live | 1 | 56018 | 0 | 2073600 |
| near-1080p | cell2_vs_frag_live | 9 | 54017 | 164 | 2073600 |
| near-1080p | frag2_vs_frag_live | 1 | 55959 | 0 | 2073600 |
| near-1080p | spike_frag_vs_main_pixel | 0 | 0 | 0 | 2073600 |
| near-1080p | spike_smooth_vs_main_smooth | 0 | 0 | 0 | 2073600 |
| near-1080p | spike_smooth_cell_lever_vs_main_smooth | 0 | 0 | 0 | 2073600 |
| near-1080p | spike_free_cell_lever_vs_main_free | 0 | 0 | 0 | 2073600 |
| near-1080p | smooth differs from pixel (sanity) | True | | | |
| probes-4k | cell_vs_frag_live | 1 | 47605 | 0 | 8294400 |
| probes-4k | cell_vs_frag_r1 | 1 | 26950 | 0 | 8294400 |
| probes-4k | cellcull_vs_cell_live | 0 | 0 | 0 | 8294400 |
| probes-4k | cell2_vs_cell_live | 1 | 208146 | 0 | 8294400 |
| probes-4k | cell2_vs_frag_live | 1 | 204366 | 0 | 8294400 |
| probes-4k | frag2_vs_frag_live | 1 | 209055 | 0 | 8294400 |
| probes-4k | spike_frag_vs_main_pixel | 0 | 0 | 0 | 8294400 |
| probes-4k | spike_smooth_vs_main_smooth | 0 | 0 | 0 | 8294400 |
| probes-4k | spike_smooth_cell_lever_vs_main_smooth | 0 | 0 | 0 | 8294400 |
| probes-4k | spike_free_cell_lever_vs_main_free | 0 | 0 | 0 | 8294400 |
| probes-4k | smooth differs from pixel (sanity) | True | | | |
| far-4k | cell_vs_frag_live | 10 | 3051 | 32 | 8294400 |
| far-4k | cell_vs_frag_r1 | 10 | 3051 | 32 | 8294400 |
| far-4k | cellcull_vs_cell_live | 0 | 0 | 0 | 8294400 |
| far-4k | cell2_vs_cell_live | 1 | 18270 | 0 | 8294400 |
| far-4k | cell2_vs_frag_live | 10 | 17585 | 32 | 8294400 |
| far-4k | frag2_vs_frag_live | 1 | 18471 | 0 | 8294400 |
| far-4k | spike_frag_vs_main_pixel | 0 | 0 | 0 | 8294400 |
| far-4k | spike_smooth_vs_main_smooth | 0 | 0 | 0 | 8294400 |
| far-4k | spike_smooth_cell_lever_vs_main_smooth | 0 | 0 | 0 | 8294400 |
| far-4k | spike_free_cell_lever_vs_main_free | 0 | 0 | 0 | 8294400 |
| far-4k | smooth differs from pixel (sanity) | True | | | |
| far-1080p | cell_vs_frag_live | 5 | 735 | 18 | 2073600 |
| far-1080p | cell_vs_frag_r1 | 5 | 735 | 18 | 2073600 |
| far-1080p | cellcull_vs_cell_live | 0 | 0 | 0 | 2073600 |
| far-1080p | cell2_vs_cell_live | 1 | 4493 | 0 | 2073600 |
| far-1080p | cell2_vs_frag_live | 5 | 4391 | 18 | 2073600 |
| far-1080p | frag2_vs_frag_live | 1 | 4600 | 0 | 2073600 |
| far-1080p | spike_frag_vs_main_pixel | 0 | 0 | 0 | 2073600 |
| far-1080p | spike_smooth_vs_main_smooth | 0 | 0 | 0 | 2073600 |
| far-1080p | spike_smooth_cell_lever_vs_main_smooth | 0 | 0 | 0 | 2073600 |
| far-1080p | spike_free_cell_lever_vs_main_free | 0 | 0 | 0 | 2073600 |
| far-1080p | smooth differs from pixel (sanity) | True | | | |
