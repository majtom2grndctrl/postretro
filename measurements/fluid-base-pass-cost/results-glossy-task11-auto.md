# Glossy world reflections, 5300M (glossy-task11-auto)

Per complete frame (gpu_time.py), mean over valid runs; spread = max - min across runs. frame = union of every labelled interval; pass sum = sum of every labelled pass's mean; overlap = pass sum - frame; chain = every Glossy Floor Chain Level pass in the frame, summed; march = Glossy Floor March Pass. Clock = median core clock (min-max) over the run's busy samples (GPU activity >= 50%), with their count, per run.

| arm | valid | overlay ms | spread | sums ms | chain ms | march ms | forward ms | spread | GPU frame ms | spread | pass sum ms | overlap ms | clocks |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| glossy-across-auto-after-m17f-high | 2/2 | 0.831 | 0.009 | 0.015 | 0.172 | 0.653 | 4.884 | 0.049 | 7.576 | 0.102 | 7.608 | 0.032 | 932 (720-1021, n=10); 942 (810-1098, n=11) |
| glossy-down-auto-after-m17f-high | 2/2 | 1.357 | 0.063 | 0.018 | 0.260 | 0.819 | 4.546 | 0.104 | 7.794 | 0.375 | 7.830 | 0.036 | 863 (642-957, n=11); 907 (349-1002, n=12) |
| glossy-across-auto-after-m17f-low | 2/2 | 0.805 | 0.026 | 0.015 | 0.000 | 0.000 | 4.757 | 0.136 | 6.630 | 0.155 | 6.669 | 0.039 | 992 (840-1012, n=9); 1007 (884-1039, n=9) |
| glossy-down-auto-after-m17f-low | 2/2 | 1.280 | 0.056 | 0.018 | 0.000 | 0.000 | 4.432 | 0.221 | 6.531 | 0.381 | 6.570 | 0.039 | 930 (833-1018, n=9); 937 (672-974, n=10) |
| glossy-across-auto-after-m17f-medium | 0/2 | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | n/a | 788 (319-947, n=13); 961 (714-1017, n=10) |
| glossy-down-auto-after-m17f-medium | 1/2 | 1.350 | n/a | 0.016 | 1.058 | 0.216 | 3.641 | n/a | 7.064 | n/a | 7.096 | 0.033 | 611 (455-804, n=14); 823 (690-972, n=10) |
| glossy-across-auto-after-m17f-very_high | 2/2 | 0.839 | 0.008 | 0.015 | 0.225 | 1.216 | 4.873 | 0.016 | 8.217 | 0.126 | 8.251 | 0.034 | 975 (829-1035, n=11); 966 (746-1026, n=12) |
| glossy-down-auto-after-m17f-very_high | 1/2 | 1.294 | n/a | 0.019 | 0.487 | 1.398 | 4.110 | n/a | 8.066 | n/a | 8.106 | 0.040 | 934 (814-1021, n=11); 848 (702-1020, n=12) |
| glossy-across-auto-after-m17f-ultra | 2/2 | 0.837 | 0.010 | 0.016 | 0.261 | 1.741 | 4.860 | 0.004 | 8.766 | 0.093 | 8.803 | 0.037 | 875 (814-1032, n=12); 916 (730-1029, n=12) |
| glossy-down-auto-after-m17f-ultra | 2/2 | 1.322 | 0.025 | 0.017 | 0.311 | 2.055 | 4.428 | 0.117 | 8.932 | 0.141 | 8.975 | 0.043 | 927 (781-1035, n=12); 876 (798-1034, n=13) |

Invalid runs:
- glossy-task11-auto-glossy-across-auto-after-m17f-medium-r1: GPU frame time differs between complete (7.8669 ms) and incomplete (8.2343 ms) frames: dropped rows lost time, not only labels
- glossy-task11-auto-glossy-down-auto-after-m17f-medium-r1: GPU frame time differs between complete (9.6214 ms) and incomplete (9.9812 ms) frames: dropped rows lost time, not only labels
- glossy-task11-auto-glossy-down-auto-after-m17f-very_high-r2: GPU frame time differs between complete (8.4193 ms) and incomplete (8.7929 ms) frames: dropped rows lost time, not only labels
- glossy-task11-auto-glossy-across-auto-after-m17f-medium-r2: GPU frame time differs between complete (7.2833 ms) and incomplete (7.5977 ms) frames: dropped rows lost time, not only labels

## Auto rerun (Task 11, HEAD ca71993bf)

Auto (1536×960) arms of the floor-only fixture, rerun in their own batch because the floor-only and tiers batches lost about half their Auto runs to the complete-frame check. Same binaries, `.prl` and settings; 2 interleaved rounds; 16/20 valid. The invalid runs are again complete and incomplete frame times differing by more than 3%. Auto Medium fails most often (0/2 across, 1/2 down), and its down run ran at a 611 MHz busy median. Auto clocks stay low and unstable (busy medians 611–1007 MHz against 1040–1190 at 1440p and 4K), as in every earlier check. The floor-only and tiers docs take their Auto figures from here.
