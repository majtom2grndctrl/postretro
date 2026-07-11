# Agentic Observability — Review Panel Findings

Captured from the post-implementation review panel run over the session diff
(`38caf3f..HEAD`, ~3,050 changed lines across `crates/`). Seven read-only lenses:
two correctness tracers on the seam refactor, one tracer + contract verifier +
adversarial tester on the feature code, one seam tracer across the driver joins,
and a hygiene/drift breadth pass.

**Verdict: no 🔴. Ship-shaped.** Full gate was green before review (fmt clean,
clippy clean on touched crates, 1474 `postretro` tests + 9 `xtask` tests pass,
no-feature build compiles with the module absent, end-to-end `xtask observe`
smoke passing byte-identically). The items below are 🟡/🟢 — correctness-as-a-
tool-surface hardening, two behavior-preservation deviations needing conscious
sign-off, and test/comment tightening. None block landing; all are cheap.

Base for all line numbers: branch HEAD at review time (post-fmt commit).

---

## Resolution status (fix pass)

All ten 🟡 findings **resolved** this pass (owner-approved; the two deviations
accepted + documented, no behavior revert):

1. Out-of-order/duplicate command ticks → rejected at parse (`RunSpecError::UnorderedCommandTick`) + tests.
2. Non-finite `wish_dir` → rejected at parse (`RunSpecError::NonFiniteWishDir`) + tests.
3. Event buffering → `TickEventRecord` construction now gated behind `dump.events`.
4. `levelLoad` order → ACCEPTED + documented (code comment + `boot_sequence.md` §3 note); splash confirmed not displaced.
5. `audio_load` order → ACCEPTED + `boot_sequence.md` §3 table updated (sound load last) + code comment.
6. Gravity → now re-read inside the tick loop (windowed parity).
7. Component-kind drift guard → rewritten to iterate `ALL_KINDS` and assert each against `ComponentValue`'s serde tag.
8. Light-before-fog drift-guard comment → corrected to state it guards segment B, not the caller's call order.
9. Dead re-exports → pruned; both `#[allow(unused_imports)]` removed; dangling intra-doc links fixed.
10. Stale "LATER" comments → updated to name `observability::driver`.

Also fixed in the same pass: `boot_sequence.md` §3 sprite-pass rows (this branch
folded two sprite passes into one — table renumbered, cross-refs updated); a
`MovementCommand` re-export warning (driver test now uses the full `runspec::` path).

🟢 nits: deferred to follow-up pass 2 (below).

## Follow-up pass 2 (Fable depth review + deferred items)

A solo Fable agent re-ran the correctness-tracer and adversarial-tester lenses over
fix pass `51cffde`. Correctness: clean (no bypass around `validate_commands`; exact
gravity parity; nothing else reads the events vec). Drift-guard rewrite: attacked,
holds (tag derives solely from variant name). Three grounded 🟢 residuals surfaced
and are now **resolved** here, along with the deferred items:

- **wish_dir out-of-range** — finite-but-huge `wish_dir` (`[1e20,0]`) passed `is_finite`
  then overflowed to a silent no-op. Now rejected at parse (`WishDirOutOfRange`,
  `|component| > 1.0`); `is_finite` check kept ahead of it so NaN is still caught.
  Stale `[0,1]` doc corrected to `[-1,1]`.
- **aim.origin unvalidated** — the symmetric twin; `origin:[1e40,..]` fed an infinite
  raycast origin. Now rejected at parse (`NonFiniteAimOrigin`, `is_finite` only —
  origin is a world position, no range bound).
- **ticks OOM (was only partially closed)** — event-gating in pass 1 fixed only the
  `events:false` case; the default `events:true` OOM was still live. Now capped at
  parse: `MAX_TICKS = 72_000` (20 min @ 60 Hz — guardrail, raise deliberately),
  `TicksExceedCap`.
- **🟢 nits:** zero-length aim → neutral yaw (was `-π`); zero-tick `facing_yaw` seeded
  from tick-0 aim; `tick >= ticks` commands now warn on stderr; `active_level_tags`
  empty-headless intentional comment; `cap:0` doc note; `mod.rs` header trimmed to
  two-line convention.
- **Pre-existing clippy blocker — RESOLVED.** `ai_tests.rs` const-assert now wrapped in
  `const { assert!(...) }` (strictly better — a const relationship belongs at compile
  time). `cargo clippy -p postretro --features observability --all-targets -- -D warnings`
  is now fully green, so all new test code is clippy-linted.

Gate (pass 2): fmt clean · check feature + no-feature clean · clippy `--all-targets`
green · 49 observability tests + `ai_tests` pass.

---

## 🟡 Should fix

### 1. Out-of-order / duplicate command ticks are silently mis-selected
**Found by 2 lenses (driver tracer + adversarial) — strong signal.**
`active_command_at` and `effective_aim_at` (`observability/driver.rs`) use
`commands.iter().rev().find(|e| e.tick <= tick)`, which is correct only if
`commands` is sorted ascending by tick. `parse_runspec` (`runspec.rs`) neither
sorts nor rejects out-of-order entries; the doc comment *claims* ascending order
but nothing enforces it.
- Input `commands: [{tick:10, forward}, {tick:0}]` → the `tick:10` command never
  activates for any tick (permanently shadowed by the reversed scan). Exit 0,
  plausible-looking output, wrong sim.
- Violates the plan's "never silently" stance for a "stable tool-facing surface."
- **Fix:** validate strictly-ascending, duplicate-free ticks at parse and reject
  with a diagnostic (non-zero exit), or sort before the loop. Prefer reject — the
  runspec is a tool contract, silent normalization hides authoring bugs.

### 2. `wish_dir` non-finite → NaN pawn position, serialized silently as `null`
**Adversarial.** `MovementCommand.wish_dir: [f32;2]` is documented as magnitudes
in `[0,1]` but never validated/clamped. `wish_dir:[1e40, 0.0]` → `Vec2(inf,0)` →
`wish_dir_from_input` normalizes `inf` → NaN wish dir → NaN velocity → NaN pawn
position. `serde_json` maps non-finite f32 to `Value::Null`, so the pawn position
emits `[null,null,null]`, exit 0. (Aim vectors are already guarded via
`normalize_or_zero` + `normalize_aim_direction`; movement is not.)
- **Fix:** reject non-finite (or clamp magnitude) on `wish_dir` at parse.

### 3. Per-tick event records buffered unconditionally, even when `dump.events:false`
**Found by 2 lenses (adversarial 🟡 + hygiene 🟢).** The tick loop always builds
and pushes a `TickEventRecord` (owned `Vec<String>` clones) every tick; the flag
is only consulted at the end in `build_output_document`. `ticks:4e9` with
`events:false` allocates billions of records → allocator abort / OOM-kill on
hostile or fat-fingered input.
- **Fix:** skip collection when `!dump.events`; consider bounding `ticks` with a
  sane cap + diagnostic.

### 4. `levelLoad` now fires before the windowed enrollment passes — behavior change
**Extraction tracer (the deepest finding).** The extraction moved the `levelLoad`
event fire *inside* segment B (`install_world_cpu`), so it now runs BEFORE the
windowed post-install steps: `absorb_dynamic_lights`, sprite/smoke registration.
Old body fired `levelLoad` dead last.
- Consequence: `absorb_dynamic_lights` enrolls untracked `LightComponent`s into
  the light bridge (install-only; a light not enrolled here is never packed →
  never rendered). Old order (`absorb` before `levelLoad`) → a light spawned by a
  `levelLoad` reaction was dropped/invisible. New order → it's enrolled and
  renders. Same theme for sprite emitters spawned by `levelLoad` reactions.
- This is a **strict improvement** (no regression direction found; despawn case is
  also cleaner), BUT it deviates from Task 1's contract "behavior-preserving …
  same install order, same side effects."
- **DECISION (owner): ACCEPT + document.** Confirmed the splash is not displaced —
  `install_level_payload` (fires `levelLoad` inside segment B, `lifecycle.rs:1190`)
  runs at `lifecycle.rs:435`; `renderer.clear_splash()` is at `:443`, after install
  returns, so the splash stays up through the whole install regardless of where
  `levelLoad` fires within it.
- **Fix (no behavior revert):** add a code comment at the `levelLoad` fire site
  noting it now precedes the windowed light/sprite enrollment passes, so
  reaction-spawned lights/emitters are enrolled (previously dropped); add a matching
  note to `context/lib/boot_sequence.md` §3.

### 5. `audio_load` step relocated to the end of install
**Found by 2 lenses (hygiene 🟡 + extraction tracer 🟢/safe).** `load_level_sounds`
+ its timing mark moved from stage ~9 (before classname dispatch) to dead last
(after `levelLoad`). Contradicts the plan's "same install order, same logs" and
`context/lib/boot_sequence.md` §3's Level Install Order table (level-sound load at
stage 9).
- Currently safe: `playSound` enqueues async `SystemReactionCommand`, drained a
  frame later, after install completes — so no reaction observes unloaded sounds.
- Fragile: any future *synchronous* audio reaction primitive touching
  `session.audio` during `levelLoad` would now run before sounds load. Also shifts
  the observable startup-timing log order.
- **DECISION (owner): ACCEPT + update docs.** Keep `audio_load` at the end of
  install; update `context/lib/boot_sequence.md` §3's Level Install Order table to
  place level-sound loading last (after the `levelLoad` event), matching the code.
- **Fix (no behavior revert):** update `boot_sequence.md` §3; add a one-line code
  comment at the `audio.load_level_sounds` call site noting the deliberate
  end-of-install position and the async-`playSound` safety that makes it safe.

### 6. Gravity read once before the tick loop; windowed re-reads it every tick
**Seam tracer.** `driver.rs` hoists `let gravity = script_ctx.gravity.get();`
above the loop; the windowed sibling (`main.rs`) calls `.gravity.get()` inside the
per-tick loop. `script_ctx.gravity` is a live `Cell<f32>` that `world.setGravity`
(a scripting primitive, callable from a reaction) mutates at runtime. A mod calling
`world.setGravity` from a reaction firing *during* the tick window is observed next
tick windowed, never headless.
- Level-load-time gravity IS captured (segment A sets it, `levelLoad` runs before
  the read). Breaks no AC and no determinism guarantee — purely a windowed/headless
  parity gap against the "one substrate, two entry points" goal.
- **Fix:** one line — move `script_ctx.gravity.get()` into the loop body.

### 7. Component-kind drift guard only tests `Health`
**Contract verifier.** `component_kind_snake_matches_component_value_serde_tag`
checks a single variant; `parse_component_kind_round_trips_every_kind` is
self-referential (round-trips the hand-written strings against themselves). A new
variant with a hand-written snake string that disagrees with serde's `rename_all`
output (e.g. `"httpproxy"` vs serde's `"http_proxy"`) passes every existing test
while the filter contract silently breaks for that kind. Violates the testing
guide's drift-guard rule (expectation must derive from the source of truth).
- **Fix:** iterate `ALL_KINDS`, serialize a representative `ComponentValue` of each,
  assert its `"kind"` tag equals `component_kind_snake(kind)`.

### 8. Light-before-fog drift guard hard-codes the order it claims to guard
**Extraction tracer.** `windowed_install_assigns_light_entity_ids_before_fog_entity_ids`
reconstructs the A→light→B sequence itself, then asserts light ids < fog ids —
reducing to "earlier-created entities get lower ids under a monotonic allocator,"
near-tautological. It does NOT drive `install_level_payload`, so a future edit that
reorders the *real* caller still passes. (The no-GPU constraint makes a faithful
end-to-end guard hard, so this may be the best feasible check — but the comment
overstates coverage.)
- **Fix:** tighten the comment to say it guards segment B's fog-after-preexisting-
  lights behavior, not the caller's call order; or extract the ordering-critical
  portion of `install_level_payload` so it can be driven headless-style.

### 9. Dead re-export surface with an inaccurate `#[allow(unused_imports)]` comment
**Found by 2 lenses (hygiene + contract verifier).** `observability/mod.rs`
re-exports a set that is only partially reachable; confirmed genuinely-unused:
`DumpSelection, EntityRecord, OutOfFrame, OutputDocument, apply_dump` (document)
and `DumpSpec, MovementCommand, RunSpec, RunSpecError` (runspec). The justifying
comment sits above only the first block (second `#[allow]` is uncommented) and
its claim ("payload structs the driver serializes transitively") is false for
`DumpSelection` (no `Serialize` derive; internal `apply_dump` return type).
- **Fix:** drop the truly-unreachable re-exports (nothing outside `observability/`
  names them via `crate::observability::X`), or rewrite the comment to accurately
  cover both blocks.

### 10. Stale "a LATER task" / "LATER driver steps" comments
**Hygiene (comment drift).** `session/mod.rs` (~:564, ~:606) describes the headless
driver as future work ("a LATER task", "LATER driver steps"). The driver shipped in
this same branch (`observability::driver::run_headless`, wired from
`startup::build_session`). An agent reading `session/mod.rs` in isolation would be
told the driver doesn't exist yet — could cause duplicated work.
- **Fix:** drop "LATER"; state the driver lives in `observability::driver` and does
  these steps (matching the accurate phrasing already two lines below).

---

## 🟢 Nits

- **Zero-tick `facing_yaw`** (driver tracer): `ticks:0` reports `facing_yaw:0.0`
  regardless of `commands[0].aim` (`last_facing_yaw` only written in the loop).
  Harmless unless zero-tick dumps are a supported use.
- **Zero aim direction → `facing_yaw = -π`** (adversarial): `aim.direction:[0,0,0]`
  → `(-0.0).atan2(-0.0) = -π`. Deterministic, non-NaN, weapon path guarded; just an
  arbitrary facing. Consider treating zero-length aim as "no aim" (keep prior).
- **Commands with `tick >= ticks` silently ignored** (adversarial): harmless no-op
  an author might want flagged.
- **`cap:0` empties the dump, reports whole population truncated** (adversarial):
  honest (count correct) but a footgun distinct from the default 1000; worth a doc
  note.
- **`active_level_tags` hard-coded empty headless** (seam tracer): defensible
  default, but any tag-gated level reaction stays inert headless — worth a one-line
  "intentional" comment so a reader tracing missing reactions isn't surprised.
- **Sidecar-present ≠ scripts-compiled** (session tracer): headless checks the
  `scripts-build` binary exists but never calls `compile_stale_scripts` (windowed
  does, before `run_mod_init`). Works in debug (`run_mod_init` compiles on the fly);
  the `None`-manifest backstop still rejects correctly. Task-4/driver territory —
  decide whether the driver should compile stale scripts to match windowed.
- **No unknown-field test for nested `aim`/`dump` structs** (contract verifier):
  `deny_unknown_fields` is present but only top-level + `movement` are tested. One
  line per nested struct guards against a refactor dropping the attribute.
- **`O(ticks × commands)` command scan** (adversarial + hygiene): both lookups
  rescan the full slice each tick. Fine for typical sizes; a cursor helps large
  runspecs. Not load-bearing.
- **`observability/mod.rs` header past the two-line convention** (hygiene): accurate
  and useful, but expands past dev-guide §5.2's orient-don't-educate rule. Trim or
  fold on next touch.
- **`nav_graph` built before the no-renderer early return** (extraction tracer):
  on the `renderer == None` error path, nav is now rebuilt before the early return
  (was skipped). Harmless, noted for completeness.

---

## Verification items (outside the diff, confirm before final landing)

- **Death-event ordering** (adversarial): the byte-identical AC depends on the
  sim's `TickEvents` lists being stably ordered. `run_death_sweep` derives from
  `report.killed_tags` — confirm no `HashSet`-ordered event list leaks into the
  collected records. Low risk (the two-identical-runs smoke passed byte-identical),
  but not traced to source in this pass.

---

## Cleared (traced, no defect) — recorded so re-review doesn't re-litigate

- Light-id-before-fog-id invariant preserved (windowed A→renderer→light→B; fog is
  first registry allocation in B; no despawns). Headless A→B yields documented
  fog-first shape.
- Host-replication registration move is safe: `is_networked_ai_map_enemy` requires
  `MapPlacement` provenance, stamped only in the archetype sweep — a `levelLoad`
  script reaction can't mint one, so the qualifying set is identical before/after.
  No double-registration (tracked-set guard), no NetworkId reordering.
- `Session::build` construction order + `script_runtime_ctor` timing unchanged by
  the `build_scripting_core` extraction.
- Manifest drain covers all four DataRegistry registrations the archetype sweep
  needs; `register_script_trees` correctly stayed windowed (needs modal_stack).
- All 15 `simulate_tick` args assembled correctly; `active_wieldable` threaded;
  `tick_dt = 1.0/60.0` (not `TICK_DURATION.as_secs_f32()`); fire rising-edge
  correct; sparse aim/movement persistence correct; `facing_yaw` derived from aim.
- No partial JSON on stdout on ANY error path (single terminal `println!`); script
  `print`/errors redirect to stderr via the logger; feature-off arm correct;
  missing vs corrupt map distinct diagnostics; held-fire not regressed.
- Registry identity: same Rc handle mutated by the tick loop and read by the dump;
  player summary built post-tick.
- Sprite two-pass → one-pass fold and model sweep CPU/GPU split are end-state
  identical; removed `App` fields (`builtin_handled`, `pending_spawn_points`,
  `pending_map_entities`) have zero remaining readers.
- Determinism/casing contracts hold: recursive nested-key sort via
  `to_deterministic_json` (sole stdout path); `ComponentKind` PascalCase derive
  untouched; `deny_unknown_fields` at every runspec level; truncation count exact
  (no off-by-one); two-category out-of-frame declaration correct (fog not
  misclassified as absent).
