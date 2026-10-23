## bounds

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| across-d45 | 3.085 / 3.179 | 0.105 | 0.047 | 0.364 |
| across-d60 | 3.148 / 3.368 | 0.115 | 0.054 | 0.360 |
| down-d45 | 3.252 / 3.432 | 0.230 | 0.161 | 0.539 |
| down-d60 | 3.413 / 3.384 | 0.270 | 0.203 | 0.535 |

## coverage

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| down-4k-ssr-high | 24.474 / 24.673 | 9.134 | — | 3.793 |
| down-4k-ssr-medium | 17.035 / 17.231 | 1.816 | — | 3.775 |
| down-4k-planar-half | 18.064 / 18.260 | 2.347 | 2.070 | 3.771 |
| down-4k-planar-quarter | 16.552 / 16.517 | 0.758 | 0.629 | 3.753 |
| down-4k-planar-full | 23.796 / 23.927 | 8.091 | 7.244 | 3.811 |

## dx12

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| ssr-high | 4.338 / 4.354 | 0.440 | — | 0.495 |
| ssr-medium | 3.890 / 3.927 | 0.107 | — | 0.491 |
| planar-half | 4.332 / 4.325 | 0.152 | 0.099 | 0.497 |

## fixtures

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| e24-across-ssr-high | 2.403 / 2.384 | 0.413 | — | 0.189 |
| e24-across-ssr-medium | 2.047 / 2.050 | 0.088 | — | 0.186 |
| e24-across-planar-half | 2.264 / 2.399 | 0.187 | 0.119 | 0.185 |
| e24-down-ssr-high | 2.510 / 2.511 | 0.541 | — | 0.310 |
| e24-down-ssr-medium | 2.077 / 2.091 | 0.115 | — | 0.309 |
| e24-down-planar-half | 2.306 / 2.291 | 0.249 | 0.177 | 0.305 |
| probes-across-ssr-high | 2.625 / 2.669 | 0.379 | — | 0.189 |
| probes-across-ssr-medium | 2.307 / 2.374 | 0.082 | — | 0.184 |
| probes-across-planar-half | 2.587 / 2.562 | 0.141 | 0.079 | 0.185 |
| probes-down-ssr-high | 2.741 / 2.711 | 0.535 | — | 0.353 |
| probes-down-ssr-medium | 2.293 / 2.234 | 0.112 | — | 0.351 |
| probes-down-planar-half | 2.684 / 2.699 | 0.205 | 0.141 | 0.356 |

## lake-sweep

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| across ssr-high | 3.384 / 3.738 | 0.426 | — | 0.356 |
| across ssr-medium | 2.993 / 2.940 | 0.106 | — | 0.358 |
| across planar-half | 3.374 / 3.188 | 0.148 | 0.088 | 0.360 |
| across planar-quarter | 3.179 / 3.128 | 0.113 | 0.056 | 0.359 |
| across planar-full | 3.413 / 3.346 | 0.277 | 0.197 | 0.364 |
| across d-30 | 3.488 / 3.107 | 0.103 | 0.045 | 0.361 |
| across d-15 | 3.064 / 3.075 | 0.085 | 0.029 | 0.359 |
| across d-8 | 3.053 / 3.186 | 0.085 | 0.029 | 0.362 |
| across r-30 | 3.254 / 3.153 | 0.130 | 0.071 | 0.360 |
| across r-15 | 3.437 / 4.476 | 0.111 | 0.049 | 0.387 |
| across r-8 | 3.196 / 3.119 | 0.078 | 0.019 | 0.359 |
| across quarter-d15-r15 | 3.507 / 3.029 | 0.070 | 0.020 | 0.358 |
| across half-d30-r15 | 3.173 / 3.120 | 0.077 | 0.018 | 0.359 |
| across fade-narrow | 3.162 / 3.128 | 0.078 | 0.018 | 0.359 |
| across fade-wide | 3.169 / 3.239 | 0.081 | 0.018 | 0.359 |
| down ssr-high | 3.541 / 3.570 | 0.682 | — | 0.545 |
| down ssr-medium | 2.939 / 2.984 | 0.151 | — | 0.540 |
| down planar-half | 3.273 / 3.295 | 0.280 | 0.216 | 0.537 |
| down planar-quarter | 3.159 / 3.298 | 0.162 | 0.098 | 0.533 |
| down planar-full | 4.218 / 3.861 | 0.732 | 0.625 | 0.541 |
| down d-30 | 3.119 / 3.319 | 0.153 | 0.084 | 0.538 |
| down d-15 | 2.990 / 2.984 | 0.085 | 0.022 | 0.536 |
| down d-8 | 3.067 / 2.996 | 0.084 | 0.022 | 0.536 |
| down r-30 | 3.415 / 3.297 | 0.287 | 0.216 | 0.535 |
| down r-15 | 3.582 / 3.365 | 0.290 | 0.216 | 0.535 |
| down r-8 | 3.492 / 3.417 | 0.278 | 0.203 | 0.537 |
| down quarter-d15-r15 | 2.991 / 3.006 | 0.085 | 0.022 | 0.538 |
| down half-d30-r15 | 3.228 / 3.229 | 0.155 | 0.084 | 0.537 |
| down fade-narrow | 3.204 / 3.112 | 0.151 | 0.084 | 0.535 |
| down fade-wide | 3.191 / 3.091 | 0.152 | 0.085 | 0.537 |

## long-bounds

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| across-d80 | 3.318 / 3.154 | 0.123 | 0.063 | 0.360 |
| across-d100 | 3.244 / 3.260 | 0.148 | 0.089 | 0.363 |

## resolution

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| 1920-ssr-high | 6.866 / 6.636 | 1.429 | — | 0.569 |
| 1920-ssr-medium | 5.516 / 5.484 | 0.317 | — | 0.566 |
| 1920-planar-half | 5.648 / 5.720 | 0.285 | 0.199 | 0.570 |
| 1920-planar-quarter | 5.683 / 5.580 | 0.157 | 0.091 | 0.566 |
| 1920-planar-full | 6.251 / 6.198 | 0.725 | 0.552 | 0.568 |
| 3840-ssr-high | 18.318 / 18.300 | 5.339 | — | 1.353 |
| 3840-ssr-medium | 14.020 / 14.000 | 1.130 | — | 1.342 |
| 3840-planar-half | 13.846 / 13.700 | 0.734 | 0.552 | 1.339 |
| 3840-planar-quarter | 13.248 / 13.212 | 0.295 | 0.200 | 1.320 |
| 3840-planar-full | 15.643 / 15.416 | 2.278 | 1.774 | 1.371 |

## tiers

| Configuration | Completion medians, ms | Reflection GPU mean, ms | Mirror forward, ms | Water composite, ms |
|---|---:|---:|---:|---:|
| ssr-off | 2.783 / 2.763 | — | — | 0.287 |
| ssr-low | 2.836 / 3.105 | — | — | 0.354 |
| ssr-very_high | 3.776 / 3.792 | 0.795 | — | 0.361 |
| ssr-ultra | 4.136 / 4.151 | 1.158 | — | 0.367 |
| planar-off | 2.793 / 2.785 | — | — | 0.289 |
| planar-low | 3.300 / 3.300 | 0.149 | 0.089 | 0.362 |

