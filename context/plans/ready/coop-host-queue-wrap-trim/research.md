# Research — host command-queue wrap-correct serial reads

Derivation notes for `index.md`. Not spec text.

## Source anchors (verified this session)

All in `crates/postretro/src/netcode/command_queue.rs` unless noted.

- `client_tick_le(a, b) = (a.wrapping_sub(b) as i32) <= 0` — `prediction.rs:41`. Wrap-aware
  serial-number `<=` (RFC 1982). Correct only for a pair within a half-range (2³¹) window;
  NOT a total order over the full `u32` range, so not a valid `Ord` for `sort`/`binary_search`.
- `pending: Vec<InputCommand>` kept in **raw `u32`** sort order.
  - `enqueue` (`:253`) inserts via `binary_search_by_key(&cmd.client_tick, |c| c.client_tick)`
    (`:266`); stale-check at `:260` uses `client_tick_le`.
  - `take_exact` (`:282`) looks up via the same raw `binary_search_by_key` (`:285`).
  - `drop_stale` (`:294`) uses `retain(!client_tick_le(c, cursor))` — order-independent.
- Catch-up trim (`:438`–`:452`): `drop_count = len − INPUT_BUFFER_TARGET`; `drain(0..drop_count)`
  drops the raw-lowest; `new_first = pending[0]` (raw-lowest survivor); `resolved_cursor =
  new_first.wrapping_sub(1)`. `diag_trimmed_jump` counts `pending[0..drop_count]`.
- First-resolution (`:460`–`:464`, the `None =>` arm): `expected = pending.first()` (raw-lowest).
- `INPUT_BUFFER_MAX = 8`, `INPUT_BUFFER_TARGET = 2`.

## The two bug sites vs. the two correct sites

The raw sort is read for two distinct purposes:

| Site | Reads sort for | Correct across wrap? |
|---|---|---|
| `enqueue` (`:266`) | exact-key insert position + duplicate detection | **Yes** — exact-key search is valid on any consistently-ordered array |
| `take_exact` (`:285`) | exact-key lookup | **Yes** — same |
| trim (`:441`,`:445`,`:447`) | **position as serial rank** | **No** |
| first-resolution (`:461`) | **position as serial rank** | **No** |

Linchpin: `enqueue`/`take_exact` never interpret position as serial order. Exact-membership
binary search and duplicate detection are correct on raw-`u32` sort regardless of the wrap.
Only the trim and first-resolution conflate raw position with serial rank. That conflation is
the entire defect.

Serial extremum over the buffered set is well-defined despite `client_tick_le` not being a
total order: `pending` holds only ticks serially ahead of the cursor (enqueue stale-drops
`≤ cursor`; `drop_stale` prunes `≤ cursor` each advance) and behind the newest received tick.
That span is « 2³¹ (a handshake/hitch backlog is tens of ticks), so a pairwise `client_tick_le`
reduction over the buffered set yields a correct serial min/max.

## Failure construction (trim)

Raw-sorted backlog `[0,1,2,3, MAX-5,…,MAX]` (len 10 > `INPUT_BUFFER_MAX`), where post-wrap
`0..3` are serially newest, pre-wrap `MAX-5..MAX` serially oldest. `drop_count = 8`,
`drain(0..8)` drops `[0,1,2,3, MAX-5..MAX-2]` — including the four serially-newest (post-wrap)
commands — keeps raw `[MAX-1, MAX]`; `resolved_cursor = MAX-2`, serially behind the dropped
post-wrap input. The exact "drop newest input / permanent latency" failure the trim exists to
prevent, plus a cursor reseated into the past → spurious flush / mis-reconcile next stream.

## Line 461 reachability (pinned this session)

`next_client_tick` is **connection-scoped**: `reset_for_level_unload` deliberately leaves it
monotonic (`prediction.rs:210`–`211`, "The outbound command tick is connection-scoped and
remains monotonic"). It resets to 0 only on a fresh `ClientPrediction` (a new connection).

The host's `ClientCommandState` (holding `resolved_cursor`) is created via
`clients.entry(client_id).or_default()` in `ingest` (cursor `None`) and torn down only by
`command_queues.remove_client`, whose two callers are:

- `host_handle_transport_disconnect` (`host.rs:439`) — genuine disconnect. The client
  reconnects with a fresh `ClientPrediction`; `next_client_tick` is back at 0. No straddle.
- `cleanup_stale_slot_replacement` (`mod.rs:1582`, called at the top of
  `host_handle_accept_descriptor_at_placement`, `host.rs:240`) — fires only when
  `slot_pawns.pawn_for(client_id)` is `Some` **and** the pawn is despawned (guards `:1595`,
  `:1598`). A normal host level change does not reach it: level unload clears `SlotPawns`
  (`host.rs:431` comment), so `pawn_for` returns `None`, it early-returns, and the queue —
  cursor and all — persists across the level change. `SlotEvent::Demoted` handling in `main.rs`
  (`:4573`, `:6055`) removes nothing.

So the cursor-`None` first-resolution path is reachable within an ongoing connection only
through a within-connection stale-pawn replacement that removes the queue while the client
keeps streaming near-`u32::MAX` ticks. Verdict: **a genuine second instance of the same
raw-sort-as-serial defect, strictly rarer than the trim** — it needs a mid-level queue teardown
*and* the wrap landing inside the freshly re-buffered set, versus the trim which needs only a
`> INPUT_BUFFER_MAX` backlog near the wrap on any live connection. A first-ever connection
cannot hit it (tick starts at 0); normal level changes cannot (queue persists). No concrete
end-to-end repro of the stale-slot-replacement path was constructed — reachable in principle
via that teardown.

Both fixes are in scope because the fix for `:461` is one line plus one test, and it closes the
same class of defect the trim fix closes rather than leaving a documented live trap.

## Alternative (b), rejected

Make `pending` serially ordered via a cursor-anchored comparator; switch `enqueue`/`take_exact`
off raw `binary_search`.

- Converts two already-correct sites for zero correctness gain (they make no serial decision).
- A cursor-anchored order is a total order only for a frozen anchor. It stays sound under
  cursor motion only because the cursor is monotonic (`wrapping_add`) AND `drop_stale` prunes
  `≤ cursor` every advance — so `pending`'s sorted-ness would become a NEW load-bearing
  invariant dependent on both facts holding forever. More fragile than raw-`u32` sort, which is
  invariant under everything.
- Still needs a fallback anchor for the cursor-`None` first-resolution case — it relocates the
  special case into the comparator rather than eliminating it.
- Touches the hot path (every `enqueue`/`take_exact`) with no perf upside: the trim bounds
  `pending` to ~`INPUT_BUFFER_MAX` (8), so a serial-extremum scan is trivial.

(a) applied to both serial-read sites is smaller, keeps storage bulletproof, and matches the
module's existing discipline — every ordering *decision* already routes through `client_tick_le`
(`enqueue` stale-check, `drop_stale`, reload observers); the trim and first-resolution are the
two position-as-rank outliers.

## Contract-doc state

`networking.md:385`–`387` currently asserts "All tick comparisons (stale-drop,
duplicate-collapse, fast-forward cursor reseat) use the wrap-aware serial-number predicate
(`client_tick_le`), correct across the u32 client_tick wrap." The fast-forward reseat's cursor
arithmetic is `wrapping_sub(1)`, but its *target selection* is raw position (`pending[0]`), so
the claim overstates today's behavior. The fix makes the claim true and adds first-resolution
to the covered set.

## Existing coverage

`client_tick_wrap_resolves_without_a_spurious_flush` (`command_queue.rs:1846`) drives a depth-1
wrap only (primes at `MAX-1`, `MAX`; ingests `0`, `1`). It never builds a `> INPUT_BUFFER_MAX`
backlog and never exercises the first-resolution straddle. Both trim and first-resolution wrap
paths are unguarded.
</content>
</invoke>
