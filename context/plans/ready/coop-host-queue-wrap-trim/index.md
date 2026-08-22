# Host command-queue wrap-correct serial reads

## Goal

The host catch-up trim and the first-resolution path read the `pending` command
queue's raw-`u32` sort *position* as if it were *serial* order. Across the `u32`
`client_tick` wrap the two diverge, so the trim drops the serially-newest commands and
reseats the resolved cursor behind them, and first-resolution starts playout at the wrong
end of a straddling buffer. Make both serial reads wrap-aware while leaving `pending`
raw-sorted, restoring the wrap-safety invariant the netcode contract already asserts.

## Scope

### In scope

- The catch-up trim in `resolve_tick` (`command_queue.rs`, the `pending.len() >
  INPUT_BUFFER_MAX` block): select and drop the serially-oldest commands, reseat the
  cursor to the serially-oldest survivor.
- The first-resolution path in `resolve_tick` (the `resolved_cursor == None` arm): pick
  the serially-oldest buffered command as the expected tick.
- An `Option`-returning serial-bounds helper on `ClientCommandState`, reusing `client_tick_le`.
- Two deterministic regression tests: a straddling-`u32::MAX` backlog of exactly
  `INPUT_BUFFER_MAX + 1` through the trim, and a first-resolution buffer straddling `u32::MAX`
  sized `2..=INPUT_BUFFER_MAX`.
- Contract-doc correction in `networking.md` (§"Host input command queue — gap policy and
  bounded playout").

### Out of scope

- Reordering `pending` storage or changing `enqueue`/`take_exact` off raw `binary_search`.
  Those sites do exact-key search and duplicate detection, which are correct on any
  consistent ordering — raw-`u32` is valid and stays. (This is alternative (b); see
  Direction.)
- The reload recovery lane (which already routes its ordering decisions through
  `client_tick_le`), and the gap-policy freeze / deep-buffer-yield / buildup-latch logic
  (count-/emptiness-keyed on `pending.len()`/`is_empty()`/`held_ticks` and cursor
  `wrapping_add` — they make no raw-`u32` ordering comparison). All wrap-clean.
- Any change to `INPUT_BUFFER_MAX` / `INPUT_BUFFER_TARGET` values or the catch-up trigger.

## Direction

**Problem.** Cause, not symptom: two sites interpret `pending`'s raw sort position as
serial rank. Raw-`u32` position equals serial rank everywhere except across the wrap, where
post-wrap ticks (small values) are serially newest yet raw-lowest. Surfaced by the review
panel on the shipped `coop-client-movement-feel` plan; it is pre-existing (the raw sort and
trim predate that plan).

**Prior commitments.** `networking.md` already asserts the queue's tick comparisons are
"correct across the u32 client_tick wrap," and the `coop-client-movement-feel` plan carries
a wrap-safety row. The trim and first-resolution reads diverge from that documented
invariant — the fix restores it rather than introducing new behavior. It also extends the
module's own discipline: every ordering *decision* in the queue already routes through
`client_tick_le` (the `enqueue` stale-check, `drop_stale`, the reload observers); the trim
and first-resolution are the two position-as-rank outliers, brought into line here.

**Alternatives rejected.** (b) Make `pending` itself serially ordered via a cursor-anchored
comparator, switching `enqueue`/`take_exact` off raw `binary_search`. Rejected: those two
sites make no serial decision, so (b) rewrites correct code for zero correctness gain; a
cursor-anchored order is a total order only for a frozen anchor, staying sound under cursor
motion only because the cursor is monotonic and `drop_stale` prunes `≤ cursor` every advance
— so `pending`'s sorted-ness would become a new load-bearing invariant more fragile than
raw-`u32` sort, which is invariant under everything. (b) also still needs a fallback anchor
for the cursor-`None` case, relocating the special case rather than removing it, and touches
the hot path for a structure the trim already bounds to ~8 elements. Keeping storage
raw-sorted deliberately avoids that one-way door and leaves (b) open should a future design
ever need O(1) serial-position semantics — for which a computed accessor would still beat
reordering storage.

Foreclosures / one-way doors: none material. The change is a localized behavior fix plus
tests; reverting is trivial, and storage representation is unchanged.

## Acceptance criteria

- [ ] A `> INPUT_BUFFER_MAX` backlog straddling `u32::MAX` (post-wrap ticks serially newest,
      pre-wrap serially oldest) run through `resolve_tick`'s trim keeps exactly the
      serially-newest `INPUT_BUFFER_TARGET` commands and drops the serially-oldest rest.
- [ ] After that trim, the resolved cursor is reseated to `(serially-oldest survivor) − 1`:
      no dropped command is serially newer than the cursor — the cursor sits at or ahead of
      every dropped tick, never behind one (in the contiguous non-wrap case the cursor equals
      the largest dropped tick; across the wrap it stays serially ahead of the post-wrap
      commands the pre-fix trim dropped). The command at reseated cursor + 1 then resolves
      `Real`.
- [ ] A buildup latch armed by a prior shallow first-resolution (one command resolved while
      `resolved_cursor == None`, which withholds `Neutral` and leaves the cursor `None`),
      followed by a `> INPUT_BUFFER_MAX` backlog on the next `resolve_tick`, resolves `Real`
      (the reseated survivor) in the same call as the trim — the post-trim disarm fires
      (`pending.len() == INPUT_BUFFER_TARGET`), so the reseated command is not withheld as
      `Neutral`. (Wrap-independent; the test may use non-wrap ticks.)
- [ ] A first resolution (`resolved_cursor == None`) on a buffer straddling `u32::MAX`, sized
      `2..=INPUT_BUFFER_MAX` (a deeper buffer is handled by the trim before the `None` arm),
      targets the serially-oldest buffered tick (the pre-wrap tick), not the raw-lowest
      (post-wrap) tick; no serially-oldest command is drop-staled unresolved.
- [ ] A non-straddling backlog and a non-straddling first-resolution buffer behave exactly
      as before the change (serial order equals raw order, so the trim and first-resolution
      outcomes are unchanged): the non-wrap trim is guarded by
      `backlog_trim_preserves_reload_press_from_dropped_prefix` (pins the exact survivor set
      and cursor), the non-wrap first-resolution by the `prime_disarmed`-based `None`-arm tests
      (`command_on_time_resolves_real`).
- [ ] After the straddling trim, `pending` remains raw-`u32`-sorted: a fresh in-order command
      ingested post-trim queues and resolves `Real`, exercising `enqueue`/`take_exact`
      `binary_search` against the interior-trimmed buffer.
- [ ] The `diag_trimmed_jump` diagnostic counts pressed jumps among the commands the trim
      actually drops (the serially-oldest set), captured before removal — not a raw-position
      prefix. Verified by code inspection (the counter feeds `netdiag` and has no test getter).
- [ ] `networking.md` §"Host input command queue" states the trim keeps the serially-newest
      commands and reseats the cursor forward across the wrap, and its tick-comparison
      summary covers first-resolution tick selection.

## Tasks

### Task 1: Wrap-correct the two serial reads + regression tests

In `crates/postretro/src/netcode/command_queue.rs`, add an `Option`-returning serial-bounds
helper on `ClientCommandState` — over the current `pending` set, the raw index of the
serially-oldest and serially-newest commands via `client_tick_le` pairwise reduction (fold
tracking the element for which `client_tick_le(c, acc)` holds / fails). It returns `None` on
an empty `pending` (the first-resolution site relies on that; see below). This is sound
because `pending` spans « 2³¹ ticks — it holds only ticks serially ahead of the cursor
(`enqueue`'s stale-check and `drop_stale` prune everything `≤ cursor`) and behind the newest
received, and the cursor drains every non-empty tick, so a near tick and one 2³¹ ahead cannot
coexist; a `client_tick_le` reduction over the set is therefore a valid total order. Then fix
the two position-as-rank reads in `resolve_tick`:

1. **Catch-up trim** (the `state.pending.len() > INPUT_BUFFER_MAX` block, currently
   `drain(0..drop_count)` + `pending[0]` reseat + `diag_trimmed_jump` over `pending[0..
   drop_count]`): rank each buffered command by serial distance behind the serially-newest
   (`newest.client_tick.wrapping_sub(c.client_tick)`); the `INPUT_BUFFER_TARGET` smallest
   distances are the survivors, the rest are dropped. A single serial-min/max cannot name the
   `INPUT_BUFFER_TARGET`-newest survivor set nor the reseat target (the *second*-newest for
   `TARGET = 2`) — the distance ranking (or equivalently rotating the raw-sorted buffer at the
   serial-min pivot and keeping the last `INPUT_BUFFER_TARGET`) is what selects them. Capture
   the dropped set *before* removal and count its pressed jumps into `diag_trimmed_jump`
   (`retain` does not hand back the removed items). Remove the dropped commands with an
   order-preserving operation so `pending` stays raw-`u32`-sorted for `enqueue`/`take_exact`.
   Reseat `resolved_cursor` to `(serially-oldest survivor).wrapping_sub(1)` — the survivor with
   the largest serial distance. Keep the existing `held_ticks = 0` reset.
2. **First resolution** (the `resolved_cursor == None` arm, currently
   `state.pending.first().map(|c| c.client_tick)?`): set `expected` to the serially-oldest
   buffered tick via the helper, preserving the `?` early-return on empty `pending` (that
   `None` is the documented "never sent a command, no prior resolution" contract).

Add two `#[cfg(test)]` regression tests in the module: (a) build a straddling-`u32::MAX`
backlog of exactly `INPUT_BUFFER_MAX + 1` commands (post-wrap ticks the serial-newest, pre-wrap
the serial-oldest — this also exercises the `MAX + 1` trim boundary), drive the trim, assert the
survivors are the serially-newest `INPUT_BUFFER_TARGET`, the reseated cursor is at or ahead of
every dropped tick, the next `resolve_tick` returns `Real`, and a fresh in-order command
ingested after the trim queues and resolves `Real` (proving `pending` stayed raw-sorted through
the interior trim); (b) prime a first-resolution buffer straddling `u32::MAX` sized
`2..=INPUT_BUFFER_MAX` (a deeper buffer routes to the trim, never the `None` arm), assert the
first resolved tick is the serially-oldest (pre-wrap) command and no pre-wrap command is dropped
unresolved; (c) drive the armed-latch → same-call-trim path: ingest one command and
`resolve_tick` once (arms the buildup latch at depth 1, withholds `Neutral`, cursor stays
`None`), then ingest `> INPUT_BUFFER_MAX` more and `resolve_tick` once, asserting the result is
`Real` at the reseated survivor tick — not the withheld `Neutral` — proving the disarm fired in
the same call as the trim (non-wrap ticks suffice). Reuse the module's
`command`/`ingest`/`resolve_tick` test helpers. Neither the
helper nor the fixes may alter `enqueue`, `take_exact`, `drop_stale`, or the
gap-policy/buildup/reload logic. Behavior on non-straddling inputs must be identical to today
(serial order equals raw order there); confirm by keeping the existing queue tests green
(`backlog_trim_preserves_reload_press_from_dropped_prefix`,
`startup_backlog_converges_and_stays_bounded`, `mid_session_hitch_catches_up`, and the
`prime_disarmed`-based `None`-arm tests).

### Task 2: Contract-doc correction

Update `context/lib/networking.md` §"Host input command queue — gap policy and bounded
playout". In the catch-up description, state that the trim keeps the serially-**newest**
`INPUT_BUFFER_TARGET` commands (not a raw-position slice) and reseats the cursor to one serial
tick behind the serially-oldest survivor, correct across the `u32` wrap. In the closing
tick-comparison summary (currently "stale-drop, duplicate-collapse, fast-forward cursor
reseat"), make the fast-forward entry accurate — the reseat selects its target by serial
order, not raw position — and add first-resolution tick selection to the wrap-covered set.
Follow `context_style_guide.md`: describe the contract in prose, name no line numbers or
internal helper names.

## Sequencing

**Phase 1 (sequential):** Task 1 — the code fix and its regression tests; establishes the
behavior the doc describes.
**Phase 2 (sequential):** Task 2 — contract-doc correction, reflecting the shipped behavior.

(Two phases only because the doc should describe landed behavior; the tasks touch disjoint
files and carry no code dependency.)

## Rough sketch

- Helper: `ClientCommandState::serial_bounds(&self) -> Option<(usize, usize)>` — raw indices
  of serial-oldest and serial-newest in `pending`, `None` when empty. `client_tick_le` fold:
  track the element `c` for which `client_tick_le(c.client_tick, acc)` (oldest) /
  `!client_tick_le(...)` (newest). Return indices, not `&InputCommand`, so the trim can select
  the survivor set from the newest anchor (a bare-reference min/max cannot name the
  `INPUT_BUFFER_TARGET`-newest set nor the second-newest reseat target).
- Trim: with `newest` from the helper, serial rank `= newest.client_tick.wrapping_sub(c.client_tick)`
  per command; the `INPUT_BUFFER_TARGET` smallest ranks survive, the rest are dropped. `pending`
  is tiny (≤ backlog size), so an index sort or partial selection is fine. Count jumps over the
  dropped set (captured pre-removal), then remove by a survivor index set (order-preserving, so
  raw sort holds). Oldest survivor = the survivor with the largest rank.
- First resolution: `expected = pending[serial_bounds()?.0].client_tick` in place of
  `pending.first().map(|c| c.client_tick)?` — same `?` early-return on empty.
- Cursor reseat: `resolved_cursor = Some(oldest_survivor_tick.wrapping_sub(1))`.

## Invariants

| Invariant | Established by | Preserved / threatened at | Verified by |
|---|---|---|---|
| After a trim the cursor is at or ahead of every dropped tick, never serially behind one | Task 1 (trim reseat to serial-oldest survivor − 1) | The trim block; threatened by any raw-position reseat | AC 2 |
| Playout begins at the serially-oldest buffered command | Task 1 (first-resolution serial-oldest) | The `resolved_cursor == None` arm | AC 4 |
| `pending` stays raw-`u32`-sorted, so `enqueue`/`take_exact` exact-key binary search stays valid | Task 1 (order-preserving removal in trim; first-resolution is read-only) | Trim removal; threatened by any reorder | AC 6 |
| Every queue ordering decision routes through `client_tick_le` (now including trim selection and first-resolution) | Task 1 | Trim + first-resolution; the rest of the module already complies | AC 1, 4 |
| `pending` span < 2³¹, so `client_tick_le` is a valid total order over the set — every path leaving ≥ 2 elements buffered advances the cursor; the non-advancing paths (buildup withhold, frontier freeze) run only at depth ≤ 1 / empty, where no far pair can coexist. Combined with monotonic 60 Hz emission, a near tick and one ≥ 2³¹ ahead cannot coexist | Existing playout loop; relied on by Task 1's helper | The serial-bounds fold | Design-established (drain/emission); fold exercised by AC 1 |
| The trim leaves exactly `INPUT_BUFFER_TARGET` survivors, so a buildup latch left armed by a prior first-resolution disarms in the same `resolve_tick` call (`pending.len() >= INPUT_BUFFER_TARGET`) and the reseated command resolves `Real` — the trim never strands a `Real` behind an armed latch | Task 1 (trim survivor count) + existing disarm check | Trim → disarm → `take_exact` handoff in one call | AC 3 (armed-latch same-call `Real`) |

## Orderings

| Scenario | Ordering | Expected |
|---|---|---|
| Backlog `> INPUT_BUFFER_MAX`, no wrap | raw order = serial order | Trim keeps newest `INPUT_BUFFER_TARGET`, reseats forward — unchanged from today |
| Backlog straddling `u32::MAX`, exactly `INPUT_BUFFER_MAX + 1` | serial-newest are raw-lowest (post-wrap) | Trim keeps the serial-newest `INPUT_BUFFER_TARGET`; cursor reseated at or ahead of every dropped tick; a post-trim in-order ingest still queues/resolves `Real` |
| First resolution, no wrap | raw-first = serial-min | `expected` = serial-oldest — unchanged from today |
| First resolution straddling `u32::MAX`, sized `2..=INPUT_BUFFER_MAX` | serial-min is raw-highest (pre-wrap) | `expected` = the pre-wrap (serial-oldest) tick; post-wrap survivors not drop-staled; buildup latch arms then disarms same-call to a `Real` |
| First resolution, `pending` empty (`resolved_cursor == None`, client sent nothing) | n/a | Helper returns `None`; the `?` early-returns; `resolve_tick` returns `None`; no panic |
| First "buffer" `> INPUT_BUFFER_MAX` (`resolved_cursor == None`, never previously armed) | trim runs before the `None` arm | Trim handles it (sets the cursor); the `None` arm and buildup latch are never reached |
| Buildup latch armed by a prior shallow first-resolution, then `pending > INPUT_BUFFER_MAX` | one `resolve_tick`: trim reseats + leaves exactly `INPUT_BUFFER_TARGET` → disarm fires post-trim → `take_exact` | Reseated survivor resolves `Real` in the same call — not the withheld `Neutral`; the trim never strands a `Real` behind the armed latch |
| `drop_stale` after a trim `Real` | survivors `{oldest S, newer N}` | `take_exact(S)` → `Real`; `drop_stale(S)` retains `N` (`client_tick_le(N, S)` false) — no survivor dropped |

## Open questions

None. The first-resolution (`resolved_cursor == None` arm, `pending.first()`) reachability was
pinned during drafting (see `research.md`): a genuine second instance of the defect, strictly
rarer than the trim, reachable in principle via a within-connection stale-pawn-replacement
queue teardown. Both sites are in scope because the marginal cost of the first-resolution fix
is one line plus one test.
</content>
