# Weapon activations — verification

Branch: `codex/weapon-activations`.
Baseline: `1c7bda3c7`.
Status: automated checks, code review and final runnable build passed. Manual visual/audio proof remains open.

## Behavior

- Primary and optional secondary actions share resources and recovery.
- Authors compose timed shots with shot/wait data. Burst fire is not an engine verb.
- Charge releases explicitly. Authors choose damage, resource and other supported scale expressions. The plasma demo’s 10× damage is content tuning.
- Host owns co-op resources, shot authorization and damage. Shot identity includes pawn, initiating controller tick, lane and ordinal. Host fire timing remains separate.
- Client catch-up preserves every due shot. Charge corrections update only the matching live flight and captured weapon instance.
- Remote detonation remains a separate tracked-deployables roadmap item.

## Automated proof

| Check | Result |
|---|---|
| Formatting | Passed |
| Production lint, warnings denied | Passed |
| Full workspace tests | 9,205 passed, 0 failed, 40 ignored across 60 targets |
| Conditioned activation transport | 8 passed, including 45–105 ms one-way delay and 5% loss |
| Local identity after switch/wrap | 1 focused regression passed |
| Captured instance and active slot reuse | 1 focused regression passed |
| SDK malformed input and valid expressions | 2 actual QuickJS/Luau helper tests passed |
| Generated SDK types | 38 passed |
| Installed scale evaluation/validation | Zero allocations in the focused probe |

The transport fixture exercises real packets, queues, admission, simulation, resource debit, reliable outcomes and client correction. It covers accepted backlog charge clamping, exact authored cadence, delayed/duplicate claims, expiry, overflow and stale projections. It does not replace manual visual/audio testing.

## Review

The panel partitions the feature into 20 slices. Each logic slice has depth review and independent breadth review. Three end-to-end traces cover authority/settlement, authored tuning/install/execution, and shot provenance/presentation. Repairs have focused tests and independent rechecks. Code review verdict: approve, with no open in-scope findings. Manual acceptance remains separate.

Resolved findings include competing cancellation order, HIT-only refusal versus FIRE denial, retained authorization after HIT retirement, presented aim consistency, projectile registry borrow lifetime, malformed SDK helper shapes, local identity across weapon switches, and incomplete network/slot-reuse fixtures.

A pre-existing client Holding transition can retain unmapped predicted flights. It is recorded in the separate `client-holding-projectile-cleanup` draft; no new damage-authority defect was established.

## Performance

Release fixture: one pawn, primary hitscan weapon and stationary target; no AI, movers or scripts. 6,300 fixed ticks: 300 warmup and 6,000 measured. Each run asserts 900 shots, final ammo 1,148 and target health 91,000. Baseline and candidate use the same measured body. Fifteen matched pairs alternate first-run order.

Machine: Intel i9-9980HK, 32 GiB RAM, x86_64 macOS 26.6.2, rustc 1.98.0, release thin LTO, AC power. No concurrent Cargo during measurement. Desktop background load is uncontrolled.

The initial regression was investigated with separate stage timing. Firing work and larger component copies were the main leads. Sharing installed common sounds reduced copying cost. Measurements still show a cost; there is no zero-overhead or whole-frame/GPU claim. Final tested candidate, median of 15 runs per version:

| Metric | Baseline | Candidate | Change |
|---|---|---|---|
| Median tick | 185.98 µs | 203.80 µs | +9.58% |
| 95th-percentile tick | 415.72 µs | 471.84 µs | +13.50% |
| Mean tick | 210.59 µs | 233.93 µs | +11.08% |
| Input payload per tick | 29.000 B | 34.571 B | Excludes transport overhead |

All workload assertions passed. Earlier optimized comparisons gave +6.7–6.9% at the 95th percentile. Final candidate absolute timing stayed near those runs, while the final baseline ran faster; background-load and clock noise remain uncontrolled. The final higher result is retained. No zero-regression claim is made. Raw temporary runs and probe binaries were removed during landing; final measurements remain recorded here.

## Manual proof

The final build and launch used `cargo run -p xtask -- run --features observability,observe-live -- content/dev/maps/combat-demo.prl`. It compiled the modified diagnostic tick producer, then reached GPU/window initialization on Metal. No reference-weapon gameplay, HUD, sound or observer presentation pass is inferred from that launch or from headless tests.

Remaining check: rifle alternate three-shot cadence; plasma partial/full charge and larger fully charged shot; charge cancellation on switching; HUD clearing; owner/remote audio and effects. Use the final runnable build.
