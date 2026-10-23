# Bounded live results

Release + dev-tools/capture, ordinary streaming, Vulkan, indirect validation off, vsync launch seed off. CPU entries are five tail 120-frame window means per run; GPU entries are three tail 120-readback window means, from logs rounded to 0.01 ms. CPU and GPU windows do not align.

| Configuration | Frame means, ms | Work CPU means, ms | Mirror walk means, ms | Submit mean, ms | Reflection GPU mean, ms |
|---|---:|---:|---:|---:|---:|
| ssr-high | 4.874 / 4.781 | 4.750 / 4.665 | absent / absent | 0.704 | 0.947 |
| half-unlimited | 7.855 / 7.804 | 7.731 / 7.681 | 2.754 / 2.744 | 0.832 | 0.210 |
| half-d30 | 5.833 / 5.801 | 5.708 / 5.681 | 0.775 / 0.775 | 0.830 | 0.120 |
| half-d15 | 5.071 / 5.068 | 4.951 / 4.950 | 0.060 / 0.061 | 0.811 | 0.090 |
| quarter-d30 | 5.807 / 5.810 | 5.683 / 5.689 | 0.774 / 0.771 | 0.834 | 0.080 |
| quarter-d15 | 5.018 / 5.037 | 4.897 / 4.917 | 0.060 / 0.061 | 0.810 | 0.070 |
| half-d30-r15 | 5.664 / 5.541 | 5.542 / 5.422 | 0.611 / 0.602 | 0.822 | 0.090 |
| half-r8 | 7.804 / 7.805 | 7.681 / 7.682 | 2.774 / 2.773 | 0.833 | 0.060 |
