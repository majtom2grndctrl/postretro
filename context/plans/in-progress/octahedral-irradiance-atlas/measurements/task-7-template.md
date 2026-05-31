# Task 7 Measurement Template

Use this file for the human-assisted before/after captures required by Phase 4 / Task 7. Do not fill in guessed values; record only measured data and link captures/logs when available.

## Build Matrix

| Run | Commit/build | Map | Resolution | GPU/backend | Probe Occlusion | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| SH baseline | TBD | TBD | TBD | TBD | n/a | Pre-migration build |
| Octahedral | TBD | TBD | TBD | TBD | On | Current migration build |
| Octahedral fast | TBD | TBD | TBD | TBD | Off | `POSTRETRO_SH_FAST=1` or debug-panel toggle |

## Shader Fetch Count

| Path | Irradiance fetches per fragment/sample | Visibility/depth fetches | Notes |
| --- | --- | --- | --- |
| SH baseline forward | TBD | TBD | Manual 8-probe L2 reconstruction baseline |
| Octahedral forward, occlusion on | TBD | TBD | 8 hardware-bilinear atlas reads plus Chebyshev visibility |
| Octahedral forward, occlusion off | TBD | TBD | Chebyshev visibility fetch skipped |
| Octahedral fog | TBD | TBD | Two directional atlas reads per probe for scatter-biased fog |

## GPU Timing

Run with `RUST_LOG=info POSTRETRO_GPU_TIMING=1` and wait for at least one 120-frame average window.

| Run | `sh_compose` avg ms | `forward` avg ms | `forward` delta vs baseline | Notes |
| --- | --- | --- | --- | --- |
| SH baseline | n/a | TBD | n/a | Sampling baseline only |
| Octahedral, occlusion on | TBD | TBD | TBD | Compose has distinct timestamp pair |
| Octahedral, occlusion off | TBD | TBD | TBD | Sampling cost inferred from `forward` delta |

## VRAM Notes

| Resource set | Resident bytes/MiB | Source |
| --- | --- | --- |
| SH baseline bands | TBD | Pre-migration build/readout |
| Octahedral base atlas | TBD | Current build/readout |
| Octahedral total atlas | TBD | Current build/readout |
| Octahedral delta storage | TBD | Current build/readout |

## Human Capture Checklist

- Capture the same camera pose on the pre-migration SH build and octahedral build.
- Capture Probe Occlusion on/off on the octahedral build; near-wall softening is expected when off.
- Capture at least one animated-light scene at peak brightness to confirm indirect delta modulation.
- Capture fog volumes with nonzero `scatter_bias` to verify directional scatter remains plausible.
- Keep logs/screenshots beside this template or link them here.
