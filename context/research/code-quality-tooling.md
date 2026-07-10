# Deterministic Code-Quality Tooling

> **Read this when:** deciding which lint/check tooling to adopt, or wiring an `xtask lint` command or CI gate.
> **Method:** mini code review of two recently completed spec PRs with every finding classified by deterministic catchability, an empirical clippy/rustfmt run over the workspace, and ecosystem research verified against current (July 2026) sources.
> **Status:** research — no tooling adopted yet. Recommendations at the end.

---

## 1. What the review found, and what a tool could have caught

Two recently merged spec PRs were reviewed: **#247** (E17-C trigger command surface — implementation) and **#245** (E17-C spec authoring). Each finding was classified: could a deterministic tool have caught it with zero LLM tokens?

### Deterministically catchable (a tool exists or is trivial to build)

| Finding | Where | Tool |
|---|---|---|
| Dead code left behind: `host_resolve_movement_inputs`, `TriggerVolumeUsage::count` never used | `netcode/command_queue.rs:419`, `scripting/systems/trigger_volume_bridge.rs:82` | **stock clippy already flags these today** (see §2) |
| Constant-value assertions in tests | `renderer/render/kinematic_brush.rs:921`, `scripting/systems/ai_tests.rs:2316` | stock clippy (`assertions_on_constants`) |
| `unreachable!` panics decoding disk data in subsystem code (re-matching raw `u8`s already validated in `level-format`) | `scripting/systems/trigger_volume_bridge.rs:53,60,65` | `clippy::unreachable` (restriction lint), non-test code only |
| Swallowed `Result` via `let _ = registry.set_component(...)` — 3 sites in one PR | `trigger_system.rs:130`, `kinematic_mover.rs:227`, `trigger_volume_bridge.rs:67` | `clippy::let_underscore_must_use` + `#[must_use]` on the API |
| Per-fixed-tick heap churn (component clones with `String`s, fresh `Vec`s/`HashMap` every tick) | `trigger_system.rs:63-131`, `sim/mod.rs:132-158`, `main.rs:1845` | in-repo: extend the existing `alloc_probe::CountingAllocator` windowed zero-alloc tests to an idle sim tick |
| File grew past the split threshold (~535 non-test lines) while gaining a second responsibility | `kinematic_mover.rs` | xtask non-test-LOC ratchet (threshold from development_guide §2.1) |
| Fabricated identifier in a promoted spec: `PawnOwnerMap` (real type: `MovementOwners`) | E17-C spec, Task 3 | xtask **spec-lint**: every backticked identifier in `plans/{drafts,ready}/**` must resolve via `git grep -w` |
| Stale line anchor in a spec (`~line 201`, actual 180) | E17-C spec | same spec-lint: resolve `` `path` ~line N `` claims to ±25 lines |
| Duplicated normative block (4-verb command table appears twice; corrections must be made in two places) | E17-C spec | spec-lint: flag duplicated line-hash runs > N lines within one file |

### Judgment-only (LLM/human review still earns its keep)

- Cross-test race: process-global `OnceLock<Mutex<Vec<…>>>` gate-fire recorder written by harness tests that don't take the serializing guard → flaky exact-content assertions (`trigger_system.rs:220-251` vs `predict_reconcile_harness.rs:783`).
- Spec semantics written against phase *fields* without re-reading the driver's position-reconstruction invariants — `stop`/`reverse`/hold as specced would teleport movers; implementation silently fixed it and the spec was never updated (§1.2 violation). The effective backstop was the spec's own *mandated deterministic tests* (freeze, ε-continuity), which forced the fixes.
- Local-player `PlayerId` derived by two independent lookups (`followed_player_pawn` vs `local_movement_pawn`) — divergence silently kills local Use triggers.
- `warn_non_mover_target_once` dedup set never cleared across level loads; recycled `EntityId`s suppress the diagnostic.
- Harness asserting a fixture artifact (client has zero trigger components) as a production invariant — production clients do spawn them.
- Compiler accepts a trigger that can never fire (empty `target_tag`, unvalidated `go_to_path_node` name) with no diagnostic.
- Replicated `target_segment` has no range validation engine-side.

**The pattern:** roughly half the findings — and nearly all the *recurring* classes (dead code, swallowed Results, latent panics, per-tick allocations, oversized files, spec-anchor rot) — are mechanizable. The judgment-only half clusters around cross-module invariants and spec semantics, which is exactly where LLM review tokens should be concentrated.

## 2. Empirical baseline (this workspace, July 2026)

- `cargo fmt --all --check`: **clean** — a zero-cost gate can be turned on today.
- `cargo clippy --workspace --all-targets` (default lints): **9 warnings**, and most are residue of the exact PRs reviewed above (the two dead-code items, the two constant assertions, `useless_vec`×4, `vec_init_then_push`). A `-D warnings` CI gate needs only this small cleanup first.
- Project invariants checked mechanically:
  - **"Renderer owns GPU" holds** — `wgpu` appears in only `crates/renderer/Cargo.toml`.
  - **`unsafe` exists in exactly one approved file** — `postretro/src/alloc_probe.rs` (`GlobalAlloc` impl, `// SAFETY:` commented). A blanket `forbid(unsafe_code)` is off the table; `deny` + file-scoped allow works.
  - **`anyhow` inside the renderer** (`renderer_init_resources.rs`) — development_guide §3.3 says anyhow is top-level only. Decide: clean up, or bless init-time renderer use explicitly in the guide.
  - Many files far exceed the 600-line threshold (`postretro/src/main.rs` 7,639) — a ratchet (no file may *grow* past its recorded size while over threshold), not a hard gate.
- CI/container note: building the workspace needs system packages `libasound2-dev` (kira→cpal) and `libudev-dev` (gilrs). Any CI job or web-session SessionStart hook must install them.

## 3. Recommended tooling (researched July 2026, all stable-toolchain)

### Must-have

1. **`[workspace.lints]` in the root `Cargo.toml`** (+ `lints.workspace = true` in all 17 members). Free at compile time.
   - `rust.unsafe_code = "deny"` (not `forbid` — `alloc_probe.rs` opts out with a scoped `allow`; workspace-inherited lints can't be relaxed per-member manifest, cargo #13157).
   - `clippy.pedantic = { level = "warn", priority = -1 }` with cherry-picked `allow`s; **never** enable `nursery`/`restriction` as groups.
   - Cherry-picked restriction lints: `unwrap_used`, `expect_used` (with `allow-unwrap-in-tests = true` etc. in `clippy.toml` — encodes panic policy §6.2), `undocumented_unsafe_blocks = "deny"` (encodes §3.5's `// SAFETY:` rule), `unreachable`.
   - Keep levels at `warn` locally; escalate with `-D warnings` only in CI.
2. **`clippy.toml` `disallowed_types`/`disallowed_methods`** for API-level rules ("no `anyhow` in library crates", "no `Instant::now` outside the frame clock"). Per-crate exemption works via a `clippy.toml` in the exempt crate's directory — caveat: nearest file **replaces** the root one entirely (no merge), so exempting crates must restate shared config.
3. **cargo-deny** (`deny.toml`): the only off-the-shelf tool that enforces dependency boundaries. `bans.deny` with `wrappers` = "this crate may only be a direct dep of the listed crates":
   - `wgpu` → wrappers `["postretro-renderer", "glyphon"]` (external direct dependents must be listed too) — mechanizes "Renderer owns GPU".
   - `nalgebra` → wrappers `["parry3d", "shambler", …]` — mechanizes the glam-boundary rule.
   - Optionally `anyhow` → wrappers for the binaries. Plus `licenses`, `advisories` (subsumes cargo-audit), `multiple-versions = "warn"`. Runs in seconds, no compilation.
4. **WGSL validation as a `#[test]`**: add `naga = "29"` as a renderer dev-dependency; glob `src/shaders/*.wgsl`, run `naga::front::wgsl::parse_str` + `naga::valid::Validator`. Validates with the *exact* compiler wgpu 29 uses, sub-second for all shaders, rides along with `cargo test`. (Prefer this over installing `naga-cli` — no version drift.)
5. **typos** (crate-ci/typos, `_typos.toml` for engine jargon) — sub-second, source-aware; typos in `context/` docs directly degrade agent behavior.
6. **lychee `--offline`** over `context/`, `docs/`, `README.md` — checks local links/anchors only (fully deterministic, no network flake). Broken links between context docs are exactly the rot that burns agent tokens.
7. **cargo-nextest** for the test gate (faster, per-test isolation, would surface finding-class flakes like the gate-recorder race sooner).
8. **xtask additions** (project-specific, each a few dozen lines):
   - `spec-lint`: resolve backticked identifiers and `file:~line` anchors in `context/plans/{drafts,ready}/**` against the tree; flag duplicated blocks. Makes the review-draft-spec "codebase-anchor" pass deterministic instead of LLM-best-effort.
   - non-test-LOC ratchet per development_guide §2.1.
   - `alloc_probe` windowed zero-alloc assertions extended to an idle sim tick.

### Nice-to-have

- **cargo-shear** (unused/misplaced dependencies; stable, ~1s, `--fix`; preferred in 2026 over cargo-machete; cargo-udeps needs nightly — skip).
- **cargo-hack `--each-feature`** on a cron job (catches `dev-tools`-gated compile breakage). Not the powerset.
- taplo or cargo-sort (TOML hygiene), cargo-workspace-lints (verifies every member inherits `[workspace.lints]`), `cognitive_complexity`/`large_stack_arrays` with tuned thresholds, committed (commit messages).

### Skip

cargo-udeps (nightly), cargo-audit (cargo-deny covers it), cargo-geiger (redundant with `unsafe_code = deny`; workspace crash reports), cargo-semver-checks (nothing published), markdownlint-cli2 (sole Node dep for style-only value), dylint (custom lints pin rustc nightly internals; revisit only if `disallowed_*` + `wrappers` prove insufficient), nightly rustfmt import options.

## 4. Wiring

`xtask lint`, cheapest first so failures are instant: `cargo fmt --all --check` → `typos` → `lychee --offline` → `cargo shear` → `cargo deny check` → spec-lint/LOC-ratchet → `cargo clippy --workspace --all-targets -- -D warnings`. Shader validation and alloc-probe checks live in `cargo test`/nextest, not lint. Have xtask probe for missing tools and print the `cargo install` line.

CI (when adopted): `dtolnay/rust-toolchain@stable` + `Swatinem/rust-cache@v2` (`save-if` main only); install tool binaries via `taiki-e/install-action` (prebuilt, seconds); `apt-get install libasound2-dev libudev-dev`; parallel jobs — cheap-checks (< 1 min, no rust cache) / clippy / nextest. CI can run `cargo run -p xtask -- lint` for exact local/CI parity.

## 5. Security scanning

> **Ownership decision:** `unsafe` is a **hard requirement** — `rquickjs` embeds QuickJS as C and `mlua` embeds Luau. We treat the safety of those libraries' internals as **their** maintainers' responsibility; auditing QuickJS's C is out of scope. `[workspace.lints.rust] unsafe_code = "deny"` still governs *our* crates (scoped `allow` in `alloc_probe.rs`) — the VM C code is an external dependency, not first-party unsafe. Postretro's security interest is the **FFI marshalling layer**: how script values are converted into engine data and interacted with at runtime, not the VM internals.

Rust removes most classic scanner targets (memory-safety, injection), so the deterministic security surface here is narrow and specific: **dependency advisories** plus **malformed-input robustness of the code that crosses a trust boundary** — the script→data FFI marshalling, the `.prl` loader, and the netcode wire codec. Two tools, both selected to trial.

### cargo-deny — dependency advisories (adopt)
Already configured for dependency-boundary `bans` (§3.3); also enable its `advisories` check against the RustSec DB (~600 advisories, actively maintained — June 2026 release) and `[bans] multiple-versions`. Subsumes standalone `cargo-audit`, and surfaces *unmaintained* transitive crates — relevant given the C-backed VM stack. Seconds to run, no compilation.

### cargo-fuzz — trust-boundary parsers and the FFI marshalling layer (trial)
libFuzzer harnesses that feed arbitrary bytes to code ingesting untrusted input and assert **"returns `Err`, never panics, terminates."** This is the deterministic tool that systematically catches the panic-on-bad-input class the review found by hand (the `unreachable!` decoding disk data; the unvalidated replicated `target_segment`). Priority targets, in order of interest:

1. **Script→data FFI marshalling** — the layer *after* the VM boundary where script values become engine data that is then interacted with at runtime. Fuzz the plain-Rust side, not the live VM: the descriptor/IR readers in `crates/scripting-core/src/data_descriptors/{js,lua}/*` and `ir/`, `conv.rs`, and the serde_json transit — feed arbitrary `serde_json::Value` / IR-descriptor input and assert bounded, panic-free conversion. This exercises the guards already living there (e.g. `JSON_CONVERSION_MAX_DEPTH = 64` in `crates/entities/src/ffi.rs`, which caps recursion from deeply-nested script objects) plus the depth/shape/type-coercion paths — without needing a QuickJS context. Fast and pure: the ideal fuzz surface, and the one that matches where we actually own risk.
2. **`.prl` loader** — `from_bytes` / section decoders in `level-loader` and `level-format`. A target over raw file bytes asserting `Err`-not-panic would have caught the `trigger_volume_bridge.rs` `unreachable!`.
3. **Netcode wire codec** — the client/host wire-apply path (`net`, `netcode`). Attacker-controlled bytes over the co-op socket; assert every client-supplied value is range-validated before it drives sim (the `target_segment` gap).

cargo-fuzz needs nightly **only for the fuzz build** (libFuzzer/sanitizer instrumentation); it does not affect the stable workspace build — targets live in a `fuzz/` member excluded from the default workspace. Run on a schedule (nightly cron) with a committed seed corpus, not per-PR — fuzzing finds *over time*, so a per-PR run is theater. A found crash becomes a committed regression seed **plus** a `proptest`/unit test in the owning crate, so it stays fixed deterministically after the fuzzer moves on.

**Ride-alongs if CI lands:** GitHub secret scanning and CodeQL (Rust GA since Oct 2025, security queries added Dec 2025) — near-zero marginal cost, modest signal for a no-web engine. Sanitizers (nightly `-Zsanitizer=address/undefined`) could exercise the QuickJS C boundary *if* the script sandbox ever enters the threat model — deferred per the ownership decision above.

## 6. Where LLM review still pays

Deterministic tooling shrinks the review surface; it does not replace review. Concentrate review tokens on: cross-module invariant coupling (duplicated lookups, global state lifecycles), spec semantics vs. driver invariants (the E17-C `stop`/`reverse` class — and note the mandated-deterministic-test-per-verb pattern in specs was the thing that actually caught those; keep requiring it), test assertions that encode fixture artifacts rather than contracts, and missing boundary diagnostics. Everything in §1's first table should never reach a reviewer again.
