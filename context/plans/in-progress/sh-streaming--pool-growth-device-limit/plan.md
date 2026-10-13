# sh-streaming--pool-growth-device-limit — plan of record

mode: compact
status: active
read at: e8f607b94

## Corrections
- Source changes since brief read-at 6078fff do not change SH pools, planner or controller. Re-read current geometry, growth, install/outcome, failure/retry and frame error boundaries; Decision premises hold.
- No extra worktrees exist. Initial disk recovery cleared Cargo incremental and downloaded archive caches; owner is also running cargo clean. Owner confirmed cargo clean finished; builds remain centralized after concurrent edits.

## Delegated answers
- Fragmentation headroom: lightweight first-fit experiment tested 1.25× (refused) and 1.5× (passed with rectangular padding); select 2.25× = 1.5× × 1.5 lifetime margin, confirmed by `fixture_churn_fits_fragmentation_headroom` in the focused renderer run. Frozen writer-closure sizes: campaign-test (21, total 57,296 tiles) and stress-warren-hallway-inspection (193, total 1,041,044 tiles). Xorshift64 seeds [1, 7, 19, 43, 101, 313, 911, 2027], 20,000 steps/seed, shuffled full demand every 257 steps and both ends. Raw peak ratios 1.5054105 / 1.0726233; largest tested refused factor 1.25×.

## AC-to-proof

Numbered in the order of the brief’s Automated rows; M1 is its Manual row.

| AC | Proof | Status |
|---|---|---|
| A1 | GPU: zero-share production-load fixture exceeds old geometry capacity and installs every cluster | achievable as stated; pending |
| A2 | CPU/GPU: shared geometry cases (floor-share, whole minimum, tiny, full layer, zero share) and exact floor accounting | achievable as stated; pending |
| A3 | GPU: dense growth preserves width, height and tiles per layer | achievable as stated; pending |
| A4 | GPU: 200→256 / 257 refusal / 100→200 dense clamp cases | achievable as stated; pending |
| A5 | GPU: all sparse families clamp to tightest buffer and refuse only excess need | achievable as stated; pending |
| A6 | GPU: device-limit verdict precedes retirement in dense and every sparse family | achievable as stated; pending |
| A7 | GPU: dense growth survives sparse refusal; later retirement/limit outcomes; sparse replacement atomicity | achievable as stated; pending |
| A8 | GPU: fitting/refused clusters in both drain orders; rollback restores address high-water | achievable as stated; pending |
| A9 | GPU outcome plus controller test: refusal releases ready bytes/payload/permit exactly once | achievable as stated; pending |
| A10 | Controller: visible episodes, owner closure, device-limit priority and bucket conservation | achievable as stated; pending |
| A11 | Controller/session: reveal-frame existing miss remains unchanged and warning once | achievable as stated; pending |
| A12 | Controller: repeated leave/re-enter refusal retries and hysteresis timing | achievable as stated; pending |
| A13 | Controller: mixed read/refusal causes retain one read-failure retry budget | achievable as stated; pending |
| A14 | Controller: rapid horizon/seam/owner churn under pressure; no refused trim/overshoot | achievable as stated; pending |
| A15 | Controller: evictions alone never wake a targeted refused cluster | achievable as stated; pending |
| A16 | Controller: failed owners block all dependent work; mixed owner causes prioritize device-limit | achievable as stated; pending |
| A17 | Log capture: first cluster named once per level; later read failure still warns once | achievable as stated; pending |
| A18 | Settling/session tests: mandatory timeout, optional no hold, warning persists across session replacement and resets by level | achievable as stated; pending |
| A19 | Controller/GPU: refusal adds no evictions; pinned/lead/owner/optional residents stay sampleable | achievable as stated; pending |
| A20 | GPU/controller regression: metadata disagreement fatal; retirement deferred | achievable as stated; pending |
| A21 | GPU regression: contract GpuCapacity, identity/generation/slot overflow remain errors | achievable as stated; pending |
| A22 | Controller invalid outcome matrix rejects duplicate/overlap/foreign/missing IDs before mutation | achievable as stated; pending |
| A23 | Source inspection: floor planner delegates all atlas geometry to shared rule | achievable as stated; pending |
| A24 | Seeded CPU churn: named fixture sizes, fixed seeds/steps, full demand per seed; measure headroom and ≥1.5× margin | achievable as stated; pending |
| A25 | Source inspection/GPU: limits captured once at load and override governs every sizing/validation path | achievable as stated; pending |
| A26 | Final required-GPU focused run: POSTRETRO_REQUIRE_GPU=1; overrides ≤ device grants | achievable as stated; pending |
| M1 | Owner in-engine A/B walks: campaign-test and stress-warren hallway at default limits; no warning; initial bytes, tiles/layer, growth, copy regions/install and install CPU | external manual proof; blocks landing |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Shared geometry, captured limits, churn headroom and zero-share production-load proof (highest-risk premise) | integrating executor / renderer worker | — | complete: actual PRL zero-share load, geometry cases and seeded churn pass |
| 2 | Clamp dense and all sparse growth, distinguish refusal from contract errors, GPU ordering/rollback proof | renderer worker | 1 | complete: real GPU clamp/refusal/rollback/readback proofs pass |
| 3 | Controller refusal failure cause, accounting, retries, pressure/miss policy and focused tests | app worker | shared outcome contract | complete: 21 refusal tests passed in 219-test focused app run |
| 4 | Level-scoped warnings, diagnostic consumers, Settling/capture integration and proof | integrating executor | 2, 3 | complete: session/Settling/log proofs, capture serialization (1), Streaming tab (5) pass |
| 5 | Review-readiness, isolated review-panel/fix loop, final preflight plus required-GPU run | integrating executor | 1–4 | readiness passed; isolated review-panel next |

## Shared contracts and performance
- `ShDrainOutcome.refused: Vec<u32>` accounts for device-limit failures exclusively and is disjoint from accepted/dropped/deferred. Renderer drops refused payloads; app releases their in-drain ready charge and permit and records the latest failure cause.
- `ShMissBuckets.device_limited: u64` counts episodes beginning behind a refused cluster/owner; existing episodes never rebucket.
- Renderer-internal `GpuDeviceLimit { reason }` is only for growth limit verdicts; `GpuCapacity` continues to identify fatal contracts.
- No refusal-driven eviction or wake. Existing ordinary floor pressure remains. Retry window is the existing 2 s hysteresis measured from last refusal. Read-failure budget survives refusal and is spent only at read-failure retry.
- Capture device limits at initialization and pass them through every SH geometry/floor/growth/replacement check. Test overrides must not exceed grants.
- Frame work remains target/change driven; no waits on growth; refusal drops payload rather than parking it. Warning latch belongs to the loaded level, across SH controller replacements. Existing counters provide install/growth CPU measurements.

## External runbook
- After automated gates, run the pre-change and feature builds on campaign-test and the stress-warren hallway with identical default limits, path, GPU, feature set and diagnostics. Record initial dense atlas bytes, tiles/layer, growth counts, upload copy regions/install and install CPU; verify no device-limit warning. Preserve machine and walk details with results.
- Manual proof is not yet supplied; leave brief in-progress with test-ready status until it is.

## Verification log
- `cargo check -p postretro-renderer --tests --features dev-tools`: pass.
- `cargo check -p postretro --tests --features dev-tools,capture`: pass.
- Required-GPU growth gate: 8 passed, none skipped, AMD Radeon Pro 5300M / Metal. A subsequent full SH-focused run passed 171/172; the newly strengthened actual-PRL fixture needed canonical stencil/node closure counts. Corrected fixture; rerun pending.
- `CARGO_INCREMENTAL=0 cargo test -p postretro --bin postretro --features dev-tools,capture sh_`: 219 passed, 2 existing heavy tests ignored, 0 failed. Includes all 21 new controller refusal tests and real hash-error, level warning replacement and actual Settling→Running proofs.
- Initial focused failures were corrected fixture assumptions (sparse stride, canonical body/index counts, untargeted background residents) and the named hash error’s case. No production behavior was weakened.
- Cold rebuilds use `CARGO_INCREMENTAL=0` to contain disk use. Cargo archive restoration required normal host network access; required GPU runs use host Metal access.

- Latest focused renderer gate: `POSTRETRO_REQUIRE_GPU=1 CARGO_INCREMENTAL=0 cargo test -p postretro-renderer --lib --features dev-tools render::sh_streaming -- --nocapture`: **172 passed**, zero ignored/skipped, AMD Radeon Pro 5300M / Metal. Actual PRL→loader→manifest→zero-share initialization installs all 9 clusters together.
- Capture device-limit serialization: **1 passed**. Streaming-tab consumer: **5 passed**.
- Review-readiness gate passed: format, touched-crate checks including tests with dev-tools/capture, and the focused test counts above.
