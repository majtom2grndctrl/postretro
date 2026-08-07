# Recorded sweep — SH section footprints by probe spacing

Evidence behind the Measured basis sections of this spec and its sibling
`lighting-scale--adaptive-base-probe-density/`. Both sweeps were run in a
headless container (4 cores, 15 GB RAM, no GPU) with release `prl-build`,
warm cache, holding `--lightmap-density 0.8 --soft-shadow-samples 64` fixed
so probe spacing is the only variable. Log lines below are verbatim.

Warm-cache caveat: the compiler warns that warm SH bakes compute indirect
lighting as approximate (bounded light reach). The 2.0 m stress run
nonetheless reproduced the prior instrumentation figures byte-exactly, but a
`--release`/`--no-cache` bake may differ. No GPU was present, so every
bandwidth figure derived from these numbers is arithmetic, not a
`POSTRETRO_GPU_TIMING` reading.

## campaign-test.map

Composition: 31 baked lights (4 selected for entity shadows), 4 animated
lights, 5 fog volumes, 3 kinematic movers. NavMesh bakes: 350 regions, 423
portals, 290×449 grid @ 0.25 m.

At `--sh-probe-spacing 1.0` (the shipping default):

```
OctahedralShVolume: 57436908 bytes (194028 probes)
DirectShVolume: 6990796 bytes (194028 probes, format 1)
DirectShDeltaVolumes: 34857290 bytes (1890 CSR entries)
DeltaShVolumes: 5451890 bytes (4 animated light(s), 295 CSR entries)
AnimatedDirectShDeltaVolumes: 5451890 bytes (4 animated light(s), 295 CSR entries)
EntityShadowLights: 20 bytes (4 selected light(s))
SH footprint (...): 110188794 bytes SH, 1607684 bytes non-SH, 111796478 bytes total
```

The `format 1` tag on `DirectShVolume` is the BC6H at-rest path; note that
`OctahedralShVolume` logs no format tag at all. Bytes per probe: id 34 =
296.0, id 35 = 36.0 — the ~8.2× gap between the uncompressed and BC6H paths.

id-41 delta bytes across the sweep: 5,253,120 (2.0 m, 285 entries);
10,027,008 (1.6 m, 544); 16,404,480 (1.33 m, 890); 34,836,480 (1.0 m, 1890).

## stress-warren-maze-crates.map

Composition: 62 baked lights (61 selected), no animated lights, no fog
volumes, no movers — `no non-directional animated lights; section empty`, so
ids 27 and 45 are absent. Atypical composition; retained because its *scale*
is representative of large community maps.

At `--sh-probe-spacing 2.0`:

```
OctahedralShVolume: 40960840 bytes (138240 probes)
DirectShVolume: 4981900 bytes (138240 probes, format 1)
DirectShDeltaVolumes: 401250346 bytes (21764 CSR entries)
EntityShadowLights: 248 bytes (61 selected light(s))
SH footprint (...): 447193334 bytes SH, 11446642 bytes non-SH, 458639976 bytes total
```

id-41 delta bytes across the sweep: 401,154,048 (2.0 m, 21,764 entries);
757,002,240 (1.6 m, 41,070); 1,305,870,336 (1.33 m, 70,848); 2,578,802,688
(1.0 m, 139,909). The 1.33 m figure is 1.216 GiB, reproducing the historic
~1.22 GiB incident as a measurement rather than an anecdote.

The 1.0 m run was OOM-killed by the kernel during post-write validation
(`anon-rss:15898028kB`) after writing a 2.93 GB `.prl`; its delta byte count
was logged before the kill and is valid. Host RAM is out of scope for both
specs — recorded so the failure is not rediscovered.

## Derived relationships

- **Payload identity.** id-41 bytes = 18,432 × CSR entries, exact at all four
  spacings on both maps (= `PROBES_PER_CELL` 64 × tile 6² × 4 halves × 2 B).
- **Scaling is sub-cubic, not O(n³).** Pairwise exponents `k = ln(byte ratio)
  / ln(spacing ratio)`: stress 2.85 / 2.95 / 2.39 (2.69 overall); campaign
  2.90 / 2.66 / 2.64 (2.73 overall). Cubic extrapolation from 2.0→1.33
  overestimates by ~4.5%.
- **Per-light reach is stable across both maps.** Each selected light reaches
  ~16% of affinity cells (16.5% stress, 17.2% campaign at 2.0 m; ~15.6% at
  1.0 m), giving id 41 ≈ 47 × probes × selected_lights and id 34 ≈ 296 ×
  probes, both fitting within ~4%. These cross at ~6 selected lights, which
  is why the two fixtures reported opposite section rankings: campaign has 4
  selected lights, stress has 61.
