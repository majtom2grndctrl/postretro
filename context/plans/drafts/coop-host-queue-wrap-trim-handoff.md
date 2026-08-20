# Handoff — host command-queue catch-up trim inverts across the `u32` `client_tick` wrap

> **Status: exploration seed, NOT a spec.** This is a context handoff so a fresh session can
> pick up one open question — resolve **option (a) vs (b)** below — and then produce a one-pager
> via `draft-plan`. It is not promoted, not scheduled, and not on the roadmap.
>
> **How to use this:** read this file + the code anchors it cites, decide (a) vs (b) with the
> owner, then draft the one-pager. Everything you need to engage the decision is here; you do not
> need to re-derive the co-op movement work that surfaced it.

## Where this came from

Surfaced by the review panel on the shipped **`coop-client-movement-feel`** plan
(`context/plans/done/coop-client-movement-feel/`). It is a **pre-existing** defect — NOT
introduced by that plan; the raw-sort + trim predate it, and that plan's own wrap surface (the
frontier `pending.is_empty()` gate, depth-keyed buildup, `expected = cursor.wrapping_add(1)`,
the `client_tick_le` stale checks) is wrap-clean. The owner reviewed it at landing and deferred
it here rather than filing a follow-up, because the trigger probability is astronomically low.

## The defect (mechanism, with current anchors)

All in `crates/postretro/src/netcode/command_queue.rs` (`HostCommandQueues` / `ClientCommandState`):

- **`pending` is kept in RAW `u32` sort order.** `enqueue` (fn `:253`) inserts via
  `binary_search_by_key(&cmd.client_tick, |c| c.client_tick)` (`:266`); `take_exact` (fn `:282`)
  looks up via the same raw `binary_search_by_key` (`:285`). So the `Vec<InputCommand>` is ordered
  by the bare `u32` value.
- **But every ordering *decision* elsewhere is wrap-aware** via the `client_tick_le` helper — the
  `enqueue` stale check (`:260`), `drop_stale` (`:294`, `retain(client_tick_le)`), the reload-lane
  observers (`:216`, `:235`). This is the internal inconsistency: the container is raw-sorted, the
  semantics are wrap-relative.
- **The catch-up trim trusts the raw sort as if it were serial order** (`:438`–`:448`):
  ```rust
  if state.pending.len() > INPUT_BUFFER_MAX {          // :438  (INPUT_BUFFER_MAX = 8)
      let drop_count = state.pending.len() - INPUT_BUFFER_TARGET;   // :439  (TARGET = 2)
      ...
      state.pending.drain(0..drop_count);              // :445  drops the raw-LOWEST `drop_count`
      let new_first = state.pending[0].client_tick;    // :447  raw-lowest survivor
      state.resolved_cursor = Some(new_first.wrapping_sub(1));      // :448  reseat cursor to it
  }
  ```

**Failure construction.** A backlog `> INPUT_BUFFER_MAX` (>8) straddling `u32::MAX` — e.g. raw-sorted
`[0,1,2,3, MAX-5, …, MAX]` where post-wrap `0..3` are serially the *newest* and pre-wrap `MAX-5..MAX`
the oldest. `drop_count = 8`, `drain(0..8)` drops `[0,1,2,3, MAX-5..MAX-2]` — i.e. the four **newest**
(post-wrap) commands — and keeps raw `[MAX-1, MAX]`; `resolved_cursor` is reseated to `MAX-2`, i.e.
**behind** the dropped post-wrap input. That is the exact "drop newest input / permanent latency"
failure the trim exists to prevent, plus a cursor reseated into the past → a spurious flush /
mis-reconcile on the next stream.

**Severity: real state-corruption, astronomically rare.** Needs a `>8`-command backlog inside the
8-tick trim window at the moment `client_tick` crosses `u32::MAX` — a boundary each client hits
about **once every ~2.27 years** of continuous play at 60 Hz. Not a crash. This is why it was
deferred, not blocking.

## The core subtlety (read before picking a and b)

**A full-range `u32` cyclic order is NOT a total order.** You cannot "just sort `pending` serially"
with a plain comparator — there is no consistent `<` across the whole `u32` range (`client_tick_le`
is defined by a half-range window trick, correct for *near* ticks but not a total order over the
full range, so it is not a valid `Ord` for `sort`/`binary_search`). Any serial ordering must be
**anchored** to a live reference (the resolve cursor, or the newest-received tick) so comparisons
are wrap-relative to a point, not absolute. This constraint is what makes the two options differ.

## The decision: option (a) vs (b)

### (a) Wrap-aware trim only  *(recommended starting lean)*
Leave `pending` raw-sorted. Fix the trim (`:438`–`:448`) to select the serially-**oldest**
`drop_count` commands *relative to the cursor / newest* and drop those, then reseat `resolved_cursor`
to the serially-oldest **survivor** (not `pending[0]`). Concretely: rank the buffered ticks by
`client_tick_le` distance from the current cursor, drop the serial-oldest tail, pick `new_first` as
the serial-min of survivors.
- **Blast radius:** one function (the trim block). `enqueue`/`take_exact`/`drop_stale` untouched.
- **Pro:** minimal surface; the raw sort is otherwise correct for the ~2.3-yr-between-wraps reality;
  low risk of regressing the hot path.
- **Con:** leaves the raw-sort/wrap-semantics inconsistency in place (a latent trap for the *next*
  reader who trusts `pending[0]` = serial-oldest). Must be commented loudly.

### (b) Cursor-anchored `pending` ordering
Make `pending` itself serially ordered by anchoring the comparator to the cursor, and switch
`enqueue` (`:266`) and `take_exact` (`:285`) off `binary_search_by_key` onto that wrap-aware search
(or a small custom structure). Then `pending[0]` / `drain(0..n)` become serially correct and the
trim needs no special-casing.
- **Blast radius:** the three `pending`-maintenance sites + any assumption that `pending` is
  raw-sorted. Wider.
- **Pro:** removes the root inconsistency; every `pending` consumer becomes serially correct for free.
- **Con:** `binary_search` needs a total order; a cursor-anchored order changes as the cursor moves,
  so the sorted invariant must be re-established or the search re-derived per call — more design and
  more hot-path scrutiny. Higher chance of a subtle new bug than (a).

**Owner's stated lean at landing:** (a) — smallest surface, and the raw sort is fine given the wrap
cadence. Revisit if (b)'s root-cause cleanliness is judged worth the surface.

## Existing coverage & what the one-pager must add

- The only wrap test today, `client_tick_wrap_resolves_without_a_spurious_flush` (in the
  `command_queue.rs` test module), exercises a **depth-1** wrap only — it never drives a backlog
  across the boundary, so the trim path is **unguarded**. Grep it for the current shape.
- **AC to add:** a deterministic straddling-backlog regression test — construct a `>INPUT_BUFFER_MAX`
  backlog crossing `u32::MAX`, run the trim, assert it keeps the serially-**newest** and reseats the
  cursor **forward** (never behind the dropped input). This is the falsifier for whichever option
  ships.
- **Contract doc:** update `context/lib/networking.md` §"Host input command queue — gap policy and
  bounded playout" (the catch-up-trim description) to state the wrap-correct trim behavior — same
  section the `coop-client-movement-feel` work rewrote.

## Pointers
- Code: `crates/postretro/src/netcode/command_queue.rs` (anchors above; `client_tick_le` is the
  wrap-aware comparator to reuse).
- Prior art / surrounding invariants: `context/plans/done/coop-client-movement-feel/` (index.md
  Invariants + Orderings, esp. the wrap-safety row) and its `research.md`.
- Router: `context/lib/index.md` → Netcode → `context/lib/networking.md`.
