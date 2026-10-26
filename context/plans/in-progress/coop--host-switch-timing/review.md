# Independent review panel — host switch timing

Target: all changes `a363ea550..05dd08f39`, 46 files, 4,858 changed lines. INCOMPLETE; no approval. Coordinator only triages/aggregates. No Cargo or source edits.

## Stat-only partitions and triage

A: changed crates/net/**, crates/netcode/** and plan.md (~1.2k). Declaration intake → retained switch/frontier → timed lower/repoint/retarget → ordered replies. Contract: declaration kinds/codec/version, exported host switch APIs. Exits to sim helpers and production host resolution. Required tracer + hygiene + contract + adversarial. Tracer and hygiene complete within their reported coverage; contract partial.

B: changed sim/**; all remaining postretro integration/adapters/old test modifications; switch_timing_fixture.rs (~1.1k). Host resolution → equip command flags → remote machine/shared countdown → reload/fire outcome. Contracts: equip/countdown helpers and command shape. Exits to A admission and C/D observations. Required tracer + hygiene + adversarial/helper contract verification.

C: switch_timing.rs + switch_timing_boundaries.rs (~1.13k). Conditioned owner commands → outcome settlement → owner/observer replication boundaries. Exits to B fixture and A timing. Required tracer + hygiene.

D: switch_timing_lifecycle.rs + switch_timing_ordering.rs + switch_timing_teardown.rs (~1.38k). Lower across trim, release/refill, lifecycle reset → settled outcomes and rejoin. Exits to A queue and B fixture. Required adversarial depth + hygiene.

Seam pass: stamped reload/start through A queues/timing → B host/sim → C/D observed outcomes. Correctness tracer required.

## Guard and pending work

Check limits before each new dispatch/workflow step; stop at <=5% remaining in either window. No reset credits. Initial primary remaining 19%, secondary 66%; last primary 15%. Disk available 63 GiB.

## Completed findings

A hygiene (Sol medium): all 20 changed net/netcode files plus plan reviewed; no confirmed code defect. Required context and full brief read. Comment drift 🔴 (hygiene, 1x): `crates/netcode/src/command_queue.rs:988–990` says “Test seam only: production resolves through [`host_resolve_remote_commands_in_registry`]”. Production instead invokes `host_resolve_remote_commands_with_switches` at `crates/postretro/src/main.rs:5154`. The old exported resolver skips timed switch callbacks, so the pointer misdirects future integration. Point documentation to timed resolver; describe legacy resolver purpose. Live text and production call verified by coordinator. Manual AC31 remains outstanding.

## Code findings

🔴 Must fix (correctness tracer, 1x): `crates/netcode/src/host_switch_timing.rs:293–301` cancels an ongoing activation for a resync to the already-active slot. Sequence: host/client both hold A, owner-private projection still names B for a second; detector permits resync during charge/burst (only pending declarations and lower/raise excluded); delivery invokes `cancel_activation()` despite same active slot, erasing the active cursor. Charge release or remaining burst shots cannot execute on host. Networking contract preserves lagging-projection resync as idempotent/no-op accept; previous implementation accepted current slot immediately. Restore same-slot accept before cancellation; add active charge/burst resync regression coverage.

## Completed coverage and stop checkpoint

A tracer (Sol xhigh): required context and complete brief read. Traced declaration intake, readiness/frontier, retained/overflow/wait/malformed release, parked lower, retarget/repoint, repair, ordered replies, client settlement and reload/start gate consumers. Changed net/netcode production paths and queue timing tests reviewed; migrated fixtures sampled. No other confirmed findings.

A hygiene (Sol medium): all 20 changed net/netcode files plus plan reviewed; no confirmed code defects, one drift finding above.

A contract (Sol xhigh): PARTIAL ONLY. Declaration enum, wire bump/gates, codec tests and timed host-apply excerpt scanned. Required material read attempts included truncated combined outputs; complete reading is not established. Client construction, retained-lane behavior and resolver API verification remain incomplete. No verified findings; this lens cannot approve.

Owner usage cutoff reached (primary 5%, then 4% remaining; weekly 63%). No additional dispatch or review steps. No reset credits consumed. Work paused with concrete findings and incomplete coverage.

## Remaining mandatory review

A contract completion and adversarial; all B/C/D depth+hygiene panels; cross-slice seam tracer; final deduplication/closure and manual AC31. No approval: request changes for confirmed activation regression, and full independent panel remains incomplete. Root owns source fixes, plan changes and continuation. No source edits, Cargo, or commits made by this coordinator/workers. Disk last checked 63 GiB available.


