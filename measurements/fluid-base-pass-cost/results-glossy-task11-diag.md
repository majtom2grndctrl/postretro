# Glossy world reflections, 5300M (glossy-task11-diag)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-1440-after-m17f-high | 1/1 | 2.047 | n/a | 0.016 | 0.320 | 1.609 | 11.192 | n/a | 16.653 | n/a | 16.703 | 0.050 | 1058 (909-1170, n=20) |
| glossy-down-1440-after-m17f-high | 1/1 | 3.055 | n/a | 0.018 | 1.233 | 1.900 | 9.452 | n/a | 16.821 | n/a | 16.869 | 0.048 | 1051 (893-1139, n=21) |
| glossy-down-1440-after-m17f-medium | 1/1 | 3.052 | n/a | 0.019 | 3.386 | 0.462 | 7.185 | n/a | 15.280 | n/a | 15.332 | 0.051 | 1091 (941-1137, n=20) |
| glossy-across-4k-after-m17f-high | 1/1 | 4.499 | n/a | 0.014 | 0.459 | 3.630 | 25.080 | n/a | 35.933 | n/a | 35.978 | 0.045 | 1199 (1044-1219, n=40) |
| glossy-across-1440-after-m17f-low | 1/1 | 1.946 | n/a | 0.016 | 0.000 | 0.000 | 10.956 | n/a | 14.357 | n/a | 14.402 | 0.044 | 1098 (290-1167, n=20) |
| glossy-down-1440-after-m17f-low | 1/1 | 2.909 | n/a | 0.018 | 0.000 | 0.000 | 10.249 | n/a | 14.337 | n/a | 14.389 | 0.052 | 1047 (922-1152, n=17) |

## Chain and forward timeline (diagnostic)

One round with the trace exports kept (`metal/raw/glossy-task11-diag-*-gpu.xml.gz`, git-ignored), 6/6 valid. Each frame's encoders are split per chain level by `levels.py`, a scratch parser over `gpu_time.py`'s parsing, frames bounded by BVH Cull Pass. Every frame records Textured Pass, then the chain levels back to back, with no gap (-0.04 to +0.006 ms).

| arm | forward | chain level 0 | levels 1+ (sum) | forward + level 0 |
|---|---|---|---|---|
| across 1440p high | 11.186 | 0.160 | 0.166 | 11.347 |
| down 1440p high | 9.266 | 1.242 | 0.175 | 10.507 |
| down 1440p medium | 6.600 | 3.841 | 0.130 | 10.440 |
| across 4K high | 25.079 | 0.240 | 0.222 | 25.318 |

Level 0 swings from 0.16 to 3.84 ms, and forward swings the opposite way by about the same amount: down 1440p reads 10.51 ms at High and 10.44 ms at Medium for forward + level 0, against forward alone at Low of 10.25. Levels 1 and up are steady at 0.13–0.22 ms. So on frames that run the floor layer, the trace attributes part of forward's work to the first chain level. That level is the first encoder to sample the highlight-free target forward writes, so forward's drain plausibly lands in it, though this trace cannot prove the mechanism. Per-pass consequences: forward alone understates the highlight-free output's cost, and the chain column overstates the chain. Read the two together.
