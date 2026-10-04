# window-modes — plan of record

mode: resumable
status: test-ready
read at: 2a4bd9eb3
branch: window-modes

Owner approved this plan in chat on 2026-10-04. The claim was pushed to `main` as `2a4bd9eb3`. Decisions and Acceptance remain the brief's contract; the identifiers below are planning labels, not rewritten acceptance.

## Corrections

- Owner amendment on 2026-10-04: unset display selection defaults to the monitor resolution at launch; Borderless disables/dims the display-mode setting to 80%; arrows browse without applying or saving; a new Apply Resolution action commits the selection. The owner's direct request authorizes this change to the prior stepping contract (A11/A15/A27/A28); the brief is updated before implementation. Task 7 implements this amendment; current focused proof, review and final preflight passed and restored test-ready status. Earlier Task 6 results remain historical; the current gate below supersedes them for changed behavior.

- `ready/game-user-dirs` has landed in `12e48599e`. The existing `startup/app_dirs.rs::AppDirs` resolves config and data directories together at stage 1, and `PendingSessionInit` already carries it. Preload from `AppDirs::settings_path()`; retain that exact path for every later save. Do not introduce a second directory resolver.
- The old `options::settings_path` cited by research no longer exists. `Session::build` takes `AppDirs`, then its private `session/mod.rs::load_player_options` reads the store and generates/saves identity. Split the pure read from post-present completion rather than copying the helper or parsing a second document.
- Promotion added explicit decided window-mode exceptions to the context docs, but their introductory boot prose still describes the current deferred read. Implement the exception and reconcile that prose at landing; audio, scripting and networking remain deferred.
- `OptionsBridge` currently suppresses effects for same-value render-resolution writes. Window-mode requests must instead consume every new write generation, even when equal to the persisted value, to satisfy P10 under escape/fallback. Engine reseeds advance the observed generation without creating a request.
- `RedrawRequested` returns early through `drive_boot_state_for_redraw` during Loading. A countdown inside `update_player_options` would miss those frames; use a monotonic deadline and service it on every redraw, including early-return paths.

## Delegated answers

- Stored display-mode key shape — five flat top-level keys: `display_mode_width`, `display_mode_height`, `display_mode_refresh_millihertz`, `display_mode_bit_depth`, `display_mode_monitor`. Their values form one optional Rust display-mode record. Integer millihertz matches winit's equality without rounding; the readonly UI refresh slot divides by 1000. Missing or malformed tuple members yield no usable display mode, preserve untouched stored text, and leave unrelated settings intact. Intentional selection writes all five keys together. Invalid `window_mode` alone loads windowed, as specified.
- macOS borderless — propose native `Fullscreen::Borderless` for both boot and live use. Local winit 0.30.13 source confirms native Space transitions, green-button readback, and that simple fullscreen refuses while native fullscreen is active. The manual boot AC explicitly permits a Space transition starting at the first frame. This is source evidence, not a runtime pass: Task 1 must verify the post-visibility boot slice before relying on it. If it fails the AC, record evidence and resolve the delegated approach within the existing contract; do not silently weaken the AC.
- Confirm identity — reserve `displayModeConfirm`, loaded from `core/ui/displayModeConfirm.json`, using the same registration boundary as `accessibilityPanel` and `initialFocus` on the revert button.
- Coordination — integrating executor owns every task, shared contract, Cargo run, plan update and commit. Implementation has overlapping boot/store/UI seams, so no implementation workers are planned. The required review-panel will dispatch its prescribed independent reviewers after integration.

## Verified source and boundary consumers

Re-read at the plan's source commit, including consumers changed since the brief's research commit:

| Contract | Current anchor and consumers |
|---|---|
| Pure load, field tolerance, document preservation, atomic save | `options/mod.rs::PlayerOptions::{load_with_status, from_table, to_document, save}`, `PlayerOptionsLoadStatus`; `options/document.rs::{StoredDocument, FieldReader, DocumentWriter}` |
| Single pre-window owner and deferred identity | `startup/session.rs::{build_session, PendingSessionInit::install, resolve_app_dirs, mod_arg, resolve_map_path}`; `startup/app_dirs.rs::AppDirs`; `session/mod.rs::{Session::build, load_player_options}`; `startup/splash_lifecycle.rs` install and first-present boundary |
| Visible boot request | `main.rs::{window_attributes, ApplicationHandler::resumed}`; renderer boot construction and first redraw request; suspend/resume retains an installed Session |
| Option working copies and all save paths | `options/graphics.rs::RenderResolution`, `options/bridge/mod.rs::{seed_on_open, observe_changes, update_with_save, seed_slot, changed_value, flush_on_options_close, flush_on_clean_exit}`, `options/bridge/save_schedule.rs`; `app/options_menu.rs::App::update_player_options`; `main.rs::exiting` |
| Every-frame ordering and early returns | `main.rs` RedrawRequested and frontend/gameplay paths; `app/render_extents.rs` ordering tests; `startup/lifecycle.rs` and boot/level modal replacement |
| Engine modal and action precedents | `app/ui_actions.rs`, `app/accessibility_panel.rs`, `options/panel_actions.rs`; `ui/actions.rs`, `ui/modal_stack/{mod,registry}.rs`, `ui/demo.rs`; `core/ui/accessibilityPanel.json` |
| Catalog and both SDKs | `entities/src/engine_state_catalog.rs` render-resolution entry; `scripting-core/src/typedef/`, `scripting-core/src/luau_require.rs`; `sdk/lib/ui/reactions.{ts,luau}`, `sdk/lib/prelude.ts`, committed SDK type snapshots; `content/dev/scripts/frontend-menu.ts` render-resolution reactions and rows |
| Actual OS behavior | Local pinned winit 0.30.13 `platform_impl/macos/{window_delegate,monitor}.rs`, `platform_impl/windows/window.rs`, `platform_impl/linux/wayland/{window/mod,output}.rs`, and `platform/wayland.rs`. Windows asserts successful display change; macOS asserts successful exclusive mode change. Wayland enumerates modes but ignores exclusive, so the engine must suppress its picker enumeration explicitly. |

No false Decision premise found. No source implementation has been changed in this planning checkpoint. The existing uncommitted `context/lib/testing_guide.md` edit belongs to the owner and is excluded from workflow commits; its test-selection guidance applies.

## AC-to-proof

Focused automated proofs below have passed; the final workspace gate is recorded separately below. Extend existing scenario coverage where appropriate rather than duplicating it. Controller tests use the production policy and an injected window adapter, deterministic monotonic time and real temporary settings files; they do not claim OS or GPU behavior. Source gates pin ownership and ordering where the real event loop cannot run in unit tests. Confirm every Cargo filter matches tests.

| AC | Acceptance (verbatim) | Proof | Status | Result |
|---|---|---|---|---|
| A1 | `window_mode` and the display mode round-trip through save and load; an absent, unknown or malformed value loads windowed for that field alone and every other setting loads intact. | Store round-trip and tolerant parsing through real temporary settings files; preserve invalid text until explicitly written | achievable as stated | pass — three store/capability tests |
| A2 | The slot vocabulary matches the store's modes, and the chokepoint maps each mode exhaustively. | Enum-to-catalog drift guard derived with exhaustive matches; compile-check exhaustive window mapping | achievable as stated | pass — catalog vocabulary/capability drift guard and exhaustive mapping |
| A3 | No fullscreen, monitor or video-mode call exists outside the window-mode chokepoint (grep gate). | Source scan of engine-owned Rust for fullscreen, monitor and video-mode calls, permitting only the window-mode chokepoint | achievable as stated | pass — all-crates window-call ownership gate |
| A4 | A stored mode present in the current monitor's enumeration is chosen; an absent one, or the same size and refresh stored for another monitor, yields borderless and leaves the stored mode unchanged. | Fresh-enumeration adapter test: full tuple match, missing mode and different monitor; assert fallback request and unchanged store | achievable as stated | pass — boot re-find/mismatched-monitor trace |
| A5 | Duplicate enumeration entries and a 0 Hz report each resolve to one choice. | Enumeration adapter test with duplicates and zero-refresh entries; assert one choice per normalized tuple | achievable as stated | pass — adapter normalization/dedup plus controller enumeration |
| A6 | An empty enumeration yields borderless, zeroed and empty picked-mode fields, and step actions that write nothing. | Empty enumeration through controller + slot projection + step actions; assert borderless, zero fields and no writes | achievable as stated | pass — empty choices projection and no-op step |
| A7 | On Wayland the enumeration reads empty and exclusive takes the fallback. | Wayland backend flag through enumeration adapter and controller; assert empty list and borderless request | achievable as stated | pass — Wayland adapter suppression and fallback trace |
| A8 | Keep before expiry persists the new mode; expiry or revert restores the prior mode and writes nothing (P4). | Controller + actual persistence test with injected monotonic time; keep wins over expiry on the same tick (P4) | achievable as stated | pass — keep/expiry/revert and actual-file persistence |
| A9 | An OS readback during a pending confirm does not persist (P3). | Pending-confirm readback through controller and persistence; assert no accepted store mutation (P3) | achievable as stated | pass — pending readback save/reload trace |
| A10 | A change to windowed or borderless persists with no confirm. | Controller + bridge save test: immediate windowed/borderless change without modal | achievable as stated | pass — immediate nonexclusive policy changes and store round-trip |
| A11 | While a confirm is pending, no save writes the unconfirmed mode, whether it came from the window-mode row or display-mode Apply: not the menu-close flush its opening triggers, not a settled save, not the exit flush; a relaunch after quitting mid-confirm boots the prior mode (P5, P6, P7). | Real-file menu-close, debounce and clean-exit saves for both request origins; reload prior mode (P5–P7) | achievable as stated | pass — both origins through close/debounce/exit saves |
| A12 | A confirm removed by a level load, restart or return to the frontend never confirms; the prior mode returns within 15 s of the change, Loading frames counted, and nothing persists (P8). | App lifecycle seam test for level load/restart/frontend replacement plus Loading early-return timing, using monotonic deadline (P8) | achievable as stated | pass — modal-instance clear/reopen, loading deadline and redraw source-order gate |
| A13 | Keep or revert with no confirm pending writes nothing and changes no mode (P9). | No-pending keep/revert through real action route; assert zero store writes and zero window requests (P9) | achievable as stated | pass — no-pending keep/revert, zero requests/store changes |
| A14 | While a confirm is pending, a display-mode step or a script write of the window mode is refused and writes nothing; revert restores the mode from before the confirm. | Pending-confirm slot and step writes through App/bridge; assert refusal and prior-mode restoration | achievable as stated | pass — refused pending requests and consumed bridge generations |
| A15 | Stepping while windowed or exclusive changes only the session-local picked display mode: no window request and no save. Apply while windowed accepts it without a window request; Apply while exclusive opens the confirm, and the stored display mode changes only on keep. Borderless refuses both stepping and Apply. | Step actions through controller + bridge + real-file reload in each effective mode; exclusive commits only on keep | achievable as stated | pass — draft-only browsing; windowed Apply commits without native request; exclusive Apply commits only on Keep; Borderless refuses both |
| A16 | The confirm opens with focus on revert. | Load shipped confirm descriptor into the real modal/focus flow; assert initial focus resolves to revert | achievable as stated | pass — shipped confirm real focus export |
| A17 | A mod or level tree registered under the confirm's name is rejected with a load-time diagnostic, and the engine confirm still shows. | Registry paths for mod/level/staged replacement plus captured diagnostic; resolve the engine confirm afterward | achievable as stated | pass — six registration routes with captured diagnostics and engine resolution |
| A18 | Settings load once per launch, before the window; no settings write precedes the first presented frame (P1, P2). | Preload-to-session ownership test with settings changed after preload; first-launch file absent until post-present completion; boot source-order guard (P1/P2) | achievable as stated | pass — preload ownership/no-reread/first-write tests and boot ordering |
| A19 | A saved borderless or exclusive mode is requested once through the chokepoint at boot, after the window is visible and before the first redraw request; the window is never created with a fullscreen attribute (grep gate) (P1). | Resume boot source-order and recorded window-call test; source scan rejects fullscreen creation attributes (P1) | achievable as stated | pass — boot source-order gate and recorded requests; no creation attribute |
| A20 | Outside a settle window, a readback differing from the baseline persists it and reseeds the working copy; an equal readback writes nothing. | Controller readback + bridge + save test for changed and equal baseline readings | achievable as stated | pass — changed/equal OS readback and pre-session regression |
| A21 | The readback reseed is not observed as a menu write and raises no live apply or second mode request. | Readback generation reseed through bridge; assert next update emits zero requests or saves | achievable as stated | pass — reseed generation feedback guard |
| A22 | While the fallback is active, readback writes nothing and the stored exclusive mode survives across frames and a relaunch. | Fallback readback across frames + actual save/reload; assert retained exclusive preference | achievable as stated | pass — fallback multi-frame save/relaunch trace |
| A23 | Readings taken mid-transition write nothing; the menu's requested mode persists, not the transition's stale reading. | Transition trace through bounded settle window and actual save; assert requested preference survives stale readings | achievable as stated | pass — stale transition trace retains requested preference |
| A24 | A request whose entry fails inside its settle window, the reading returning to windowed, writes nothing and keeps the stored mode; an OS-driven change after the window closes persists as usual. | Failed-entry settle trace followed by an OS change; assert unwritten baseline then accepted OS persistence | achievable as stated | pass — failed-entry baseline and later OS change trace |
| A25 | `--windowed` boots windowed over a saved fullscreen mode and leaves the store unchanged; a menu change in that session persists. | Boot-escape argv + preload + controller test and actual save/reload, followed by an accepted menu write | achievable as stated | pass — boot escape and later accepted slot request |
| A26 | Under `--windowed` or the fallback, choosing the saved mode itself applies it: exclusive with the confirm when the mode matches, the fallback and its warning when it does not (P10). | Same-value slot write under escape and fallback through bridge/controller; fresh matching enumeration confirms, missing match warns (P10) | achievable as stated | pass — same-value reapply, confirm/fallback and once-per-request captured Warn |
| A27 | The Scripting surface example runs as `content/dev` options-menu rows: stepping changes the shown size and refresh, and the window-mode rows write the slot. | Compile/evaluate the content/dev menu via the real script/UI seam; activate rows and steps against injected modes and inspect bindings | achievable as stated | pass — shipped UI activation → reaction/action dispatch → bridge/policy → live text → actual saved choice |
| A28 | The Luau SDK carries the display-mode action helper and the window-mode and display-mode slots with the same ops, values and types as TypeScript. | TS type tests, Luau virtual-module evaluation, catalog-derived generated typedef checks and action-op parity tests | achievable as stated | pass — dual-runtime/helper/catalog parity, generated declaration guard/snapshot, focused TS typing; Luau analyzer unavailable |
| A29 | With no saved display tuple, launch seeds an enumerated monitor-resolution default; entering Exclusive uses it and shows the confirm. An unavailable saved tuple keeps its fallback behavior, and no boot seeding writes settings. | Adapter selection/default and controller boot-to-exclusive/save tests | achievable as stated | pass — adapter default selection and boot-to-exclusive/file-isolation regression |
| A30 | The dev menu disables display-mode arrows and Apply while Borderless is selected, with the entire display-mode setting drawn at 80% opacity; returning to Windowed or Exclusive enables it. | Shipped UI draw/focus/activation test with exact alpha and refused action/save assertions | achievable as stated | pass — shipped draw alpha 204, disabled focus/activation and re-enabled controls |
| M1 | Windows: boot into each saved mode; the first splash frame shows that mode. | Owner in-engine runbook below; record OS, display, GPU, build and observed outcome | manual; blocks landing | outstanding — external manual proof |
| M2 | macOS: boot into each saved mode with no windowed splash frame after the first; a native Space transition starting at the first frame passes. | Owner in-engine runbook below; record OS, display, GPU, build and observed outcome | manual; blocks landing | outstanding — external manual proof |
| M3 | macOS and Windows: switch live among all three, both directions; leaving exclusive restores the desktop resolution and the prior window size. | Owner in-engine runbook below; record OS, display, GPU, build and observed outcome | manual; blocks landing | outstanding — external manual proof |
| M4 | macOS and Windows: exclusive confirm keeps; expiry reverts. | Owner in-engine runbook below; record OS, display, GPU, build and observed outcome | manual; blocks landing | outstanding — external manual proof |
| M5 | macOS: the green button enters and leaves fullscreen; the menu reflects it, and the next launch matches. | Owner in-engine runbook below; record OS, display, GPU, build and observed outcome | manual; blocks landing | outstanding — external manual proof |
| M6 | Windows: boot reaches the frontend in every saved mode; repeat once U4 lands. | Owner in-engine runbook below; record OS, display, GPU, build and observed outcome | manual; blocks landing | outstanding — external manual proof |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin boot slice: one settings read, pending-store handoff, borderless after visibility and `--windowed` | integrating executor | — | automated complete; macOS visual result pending |
| 2 | Display-mode store, fresh enumeration/re-find policy and window slot catalog | integrating executor | 1 | complete: check + 26 matched window-filter tests |
| 3 | Live requests, engine confirm, revert lifecycle and persistence isolation | integrating executor | 2 | complete |
| 4 | OS readback, bounded settle/fallback and generation-safe reseeding | integrating executor | 3 | complete |
| 5 | TS/Luau action surface, generated types and dev options-menu rows | integrating executor | 4 | complete |
| 6 | Integration gate, review/fix loop, final preflight and platform acceptance report | integrating executor | 5 | automated complete; test-ready, M1–M6 block landing |
| 7 | Owner amendment: launch default, browse/Apply separation and disabled Borderless picker | integrating executor | 6 | complete: focused proof, four-lens review and final preflight passed; native proof outstanding |

### 1. Thin boot slice

Add the three-state store enum with windowed default and tolerant `window_mode` parsing. Carry the loaded store, load status and resolved settings path through `PendingSessionInit`; move it into `Session::build` without cloning/re-reading the settings document. Keep identity entropy, directory creation and first-launch writes in the post-present completion step. When recreating a window with an installed Session, read its in-memory settings instead of loading the file again. Parse `--windowed` at stage 1 without treating it as a map argument.

Create the app-side `app/window_modes/` owner, with CPU policy separated from the winit adapter. Every fullscreen/monitor/video-mode call belongs to this chokepoint. Apply native borderless immediately after visibility and before the first redraw request; no fullscreen creation attributes. Keep renderer extent recording/commit under its current owner.

Focused proof: store boundary tests, the preload-to-session ownership/first-write scenario, boot ordering gates, escape behavior and existing boot/session/render-extents ordering tests. Then run the borderless boot slice on macOS through `cargo run -p xtask -- run --app-name postretro-window-modes-test`, using a disposable settings file for that app name. Record whether the native Space transition starts at the first frame and boot reaches the frontend. Obtain owner visual evidence if this session cannot observe native windows. Do not report a runtime pass from source ordering alone. Check free space after the task and commit the task's actual proof with the plan update.

### 2. Display-mode store and catalog

Own store types in a focused `options/window.rs` module. Load/save the delegated tuple with the existing document-preservation mechanism. Within the chokepoint, enumerate only the window's current monitor and suppress enumeration on Wayland based on the actual event-loop backend. Normalize a reported zero refresh with the current monitor's reported refresh when available; retain zero if still unknown, never invent a rate. Deduplicate and sort by the full size/refresh/bit-depth/monitor tuple. Cache picker choices only for projection/stepping; every exclusive apply, reapply and revert must re-find a fresh handle before calling winit. A missing mode or mismatched monitor requests borderless, warns at the request boundary, and retains the stored tuple.

Add writable, non-persisted/non-replicated `options.windowMode` and the six readonly, client-local `window.displayMode*` catalog slots listed by the brief. Picked fields project the stored or pending tuple only when available in the current enumeration; an empty enumeration projects zeros/empty string and stepping is a no-op. Step in deterministic wraparound order; without a matched selected tuple, next selects the first and previous selects the last. Preserve modes differing only in bit depth.

Focused proof: A1–A7 and catalog capability/type/default assertions derived from the Rust modes. Touch the catalog mechanism minimally; do not move window policy into `entities` or introduce winit there. Check free space and commit with task proof.

### 3. Live requests and confirmation

Consume window-mode slot generations as requests before mutating the persisted store, including equal-value writes under escape/fallback. A matching live exclusive apply snapshots the prior effective mode and picked tuple, requests the fresh mode, opens the engine confirm with revert focus, and holds the candidate entirely outside `PlayerOptions`. Accepted windowed/borderless changes write and schedule save immediately. A missing exclusive match takes the session fallback without discarding the requested exclusive preference, as the brief specifies.

Register `displayModeConfirm` through the built-in core-root loader; reject mod and level registrations at the shared registry boundary with a diagnostic. Intercept exactly `ui.displayMode.next/previous/apply/keep/revert` before named-reaction dispatch; route generic cancel/close to revert when the confirm is active. Keep with no pending transaction is a no-op. Keep commits the candidate, marks its fields written and schedules the ordinary save. Revert/expiry restore the prior effective mode through fresh re-find, restore the picked fields and reseed working copies without saving. Refuse all other window-mode writes/step actions while pending, advancing consumed generations so a refused request never replays after revert.

Use an injected monotonic clock and a 15-second deadline, not clamped simulation/frame delta. Keep on the expiry tick wins after that tick's available UI actions are drained; Loading/early-return frames still service expiry. Removal of this confirm instance through level load, restart or frontend replacement invalidates it and reverts; a subsequent action cannot keep an invalid transaction. Preserve controller state across level boundaries, resolve an active transaction before a window is discarded, and never let modal-stack clearing destroy its deadline. Test both frontend and gameplay action routes.

Focused proof: A8–A19 and A26, including actual file saves through menu-close, debounce and clean-exit paths for window-row and display-step origins. Reload those files to prove the unconfirmed candidate never reaches disk. Confirm focus and registry diagnostic tests use the shipped tree. Check free space and commit with task proof.

### 4. Readback and live integration

Read actual mode once per rendered app frame through the window chokepoint, including Loading. Compare with the controller's actual baseline rather than persisted preference. Every requested entry/exit opens a fixed bounded settle interval; choose and document the interval from Task 1's transition evidence, retaining deterministic injected-time coverage. Mid-settle readings are unwritten; at closure adopt the reading as an unwritten baseline even if entry failed. Pending confirm and active fallback suppress persistence. A changed OS reading outside those guards updates the store, marks written fields, schedules save, and reseeds the matching working copy with its generation recorded. Equal readings do nothing. A deliberate menu request leaves boot-escape suppression and re-runs fresh apply even when equal to the saved preference.

Preserve `update_player_options` → `commit_render_extents` → camera assembly in gameplay and frontend paths; resize events still only record extents. Controller timing cannot hide inside those logic-only paths. Perform idle readback without enumeration, formatting or allocation: maintain enum/numeric baseline state, compare borrowed monitor data if needed, enumerate only at boot/open/step/apply, and allocate labels only on changes. Countdown writes occur only when the displayed seconds change. With CPU timing enabled compare the housekeeping stage before/after an idle frontend run on this machine when that timing path produces samples; if it does not, record the limitation and verify bounded work by inspection.

Focused proof: A20–A26, replayed requested/failure/OS traces, generation feedback guards, and existing render-extent ordering coverage. Check free space and commit with task proof.

### 5. Script and menu surface

Add `displayModeAction(op)` in both SDK implementations with the same closed five-op vocabulary. Update the TS prelude/export path and the Luau virtual UI module export allowlist; update typedef generator templates and regenerate committed TS/Luau types through the established engine type-emission path. Author the brief's window-mode rows and size/refresh step controls in `content/dev/scripts/frontend-menu.ts`, following the existing graphics tab and render-resolution reactions. The menu reads readonly picked fields and writes the working-copy slot; it never receives a mode list or accesses the store. Update relevant human SDK documentation with bundle-runnable commands only if needed to explain the new exported API.

Focused proof: A27/A28 through production script evaluation, UI activation and injected modes; TS valid/invalid op type checks; Luau helper/module parity; generated catalog slot types. Review existing accessibility/graphics fixtures before choosing whether to extend one or add a distinct display-mode scenario. Check free space and commit with task proof.

### 6. Review and acceptance

Run `cargo fmt --check`, `cargo check` for touched crates, and focused tests with nonzero matched counts. Do not run the full workspace suite beforehand. Run `review-panel`, then `fix-review-findings` and focused retests for its concrete findings; repeat only for a new concrete finding. Any finding changing Decisions/Acceptance returns to the owner. Run `preflight` once as the final gate after fixes.

Fill a result column for every AC, including outstanding external proof. The brief does not permit landing ahead of manual proof, so set `status: test-ready` while any M row is outstanding and keep it in `in-progress/`. Commit the report, branch and runbook, then obtain blocking results and the owner's separate “land the plane” direction. Landing updates durable contracts, roadmap if listed, moves the brief to done, cleans only workflow-owned artifacts/caches, and pushes the branch as specified by `build-brief`.

### 7. Owner-requested picker amendment

Seed only an absent stored tuple from an enumerated mode matching the monitor resolution at boot; prefer current refresh and deterministic bit depth. Keep unavailable saved tuples unchanged and preserve boot escape, Wayland and fresh-handle fallback. The resolved default stays in the controller, so pre-window/first-present persistence rules stay intact.

Keep draft picks outside PlayerOptions. Next/previous update only readonly projection, with no apply, confirm or save scheduling. Apply saves a windowed selection without resizing the window, or requests an exclusive choice through the existing confirmation. Selecting Exclusive itself uses the current picked/default tuple and confirms. Pending transactions refuse Apply as well as steps and mode writes. Revert restores the accepted picked fields and effective mode. Borderless refuses picker actions.

The shipped menu uses its existing conditional visibility and disabled controls for mode-specific branches; per-run color alpha gives the label, arrows, size/rate and Apply button exactly 80% opacity in Borderless, without adding a generic opacity API. All per-frame work remains cached/readback and change-driven projection; enumeration and label allocation remain at boot/menu/actions.

Proof: extend existing policy persistence/confirm tests and shipped UI activation/draw/focus scenario; adapter default selection, fresh-profile default-to-exclusive and stale saved-mode regressions; extend the existing production menu contract test and dual-runtime SDK/generated declaration guards. Run touched checks, focused tests and strict TS, then isolated review-panel and fixes/focused retests, then final preflight. Commit/push for the owner’s already-requested two-computer testing handoff; restore test-ready with native rows outstanding.

## Manual runbook (blocking external proof)

Use a disposable app name `postretro-window-modes-test` so the normal game's settings are untouched. With fresh disposable settings, close the first-launch accessibility panel normally. The existing native-trial profile was preset with `accessibility_panel_shown = true` to isolate window boot and therefore skips that hold. The dev Graphics menu should initially show the monitor resolution when no display choice is saved. Use its arrows to browse, then Apply Resolution to accept a windowed selection or test an exclusive change; Borderless disables/dims this setting. Use the dev graphics menu to select modes and enumerate actual choices; retain a copy of the test settings before each trial. For each result record OS/version, monitor/name, native resolution/rate, GPU/driver, feature-branch commit, launch command, and observed first-frame/mode/confirmation behavior. Windows evidence is external to this macOS workspace.

1. M1/M2: Save windowed, borderless and one valid exclusive choice, exit cleanly and relaunch each with `cargo run -p xtask -- run --app-name postretro-window-modes-test`. Observe the first splash frames. Windows must show the saved mode; macOS may begin its native Space transition at the first frame but may not switch from a later windowed splash. Verify `--windowed` recovers the same saved fullscreen settings without changing them.
2. M3: Switch among all three modes in both directions on macOS and Windows. Record desktop resolution before/after exclusive and window size before/after fullscreen; both must restore when leaving exclusive.
3. M4: Keep one valid exclusive change, then browse to another resolution, press Apply Resolution and allow 15 seconds to expire; also explicitly revert/cancel. Check visual restoration and restart against the persisted prior choice. Repeat with focus lost and with a level-load/restart/frontend return to exercise deadline/lifecycle behavior.
4. M5: On macOS enter and leave fullscreen with the green button, outside an app-request settle interval. Check the menu reflects each OS change and relaunch matches it without a second mode request caused by reseeding.
5. M6: On Windows verify frontend arrival for every saved mode. When E23 U4's hidden-window accessibility adapter lands, repeat this boot matrix and append its build identity and results; do not infer the future integration's success. Until U4 exists, record that repeat as outstanding explicitly.

## Resumption

Owner approval and the directly requested amendment are recorded. Tasks 1–7 are complete; the current automated acceptance, review and final preflight passed. M1–M6 remain outstanding and block landing, including M6's repeat after U4. Resume by collecting native results against the updated manual runbook, recording each result and build identity, and resolving any demonstrated failure through focused repair and verification. Do not infer native success from automated tests, the owner's general positive feedback or the earlier stalled launch trials. Once blocking proof arrives, await the owner's separate “land the plane” direction and follow the landing steps in `build-brief`; the brief stays in `in-progress/` until then. The owner's uncommitted testing-guide changes and concurrent unrelated plan edits remain excluded from workflow commits.

Check workspace free space after each numbered task/checkpoint; below 15 GB clear only Cargo incremental caches in active/workflow-owned targets, then downloaded crate archives if necessary, and recheck. Initial available space: 45 GiB; latest Task 7 checkpoint: about 27.5 GiB. Only Cargo incremental caches were cleared during this workflow; full target directories remain.

## Task proof

- Task 1: `cargo check -p postretro` passed; options filter: 61 tests; session/startup filter: 58 tests; boot-mode ordering filter: 1 test; render-extent filter: 4 tests, all passed. Sandbox launch reported macOS service errors; stopped only that workflow process and relaunched with desktop access. Disposable settings under `postretro-window-modes-test`; visual outcome is not inferred from logs.

- Task 2: `cargo check -p postretro` passed. `cargo test -p postretro --bin postretro window -- --nocapture`: 26 matched tests passed, including persistence/tolerance, current-monitor re-find, empty Wayland choices, duplicates/zero refresh, escape and the all-crates window-call ownership gate. Readback wiring is implemented in the adapter but awaits Tasks 3/4, producing temporary dead-code warnings at this checkpoint.

- Task 3: Controller filter: 8 tests; options bridge: 17 tests; render extents: 4 tests; confirm focus/action descriptor: 1 test; reserved registration boundary: 1 test, all passed. Confirmation uses an opaque modal instance, so clear/reopen cannot keep an old transaction. Borderless and windowed native boot trials both reached window/renderer initialization but did not present; both workflow processes were stopped. This does not establish a fullscreen failure or a visual pass; platform acceptance remains outstanding.

- Task 4: `cargo check -p postretro` passed without warnings; window-controller filter: 14 matched tests passed. Traces prove pending/fallback isolation through actual save/reload, stale/failed-entry baseline adoption, OS persistence without feedback requests, same-saved-value requests under escape/fallback, and zero idle projection writes. Three seconds is the provisional bounded settle interval; native timing and CPU samples were unavailable because both native trials stalled before first presentation. The manual runbook must validate the bound. Idle inspection: one cached fullscreen read, no mode enumeration or formatting, no allocation for an equal reading or unchanged projection.

- Task 5: Production TypeScript menu evaluation and TS/Luau SDK parity: 2 tests passed; compiler-time Luau UI export: 1 test; generated SDK snapshot drift: 1 test; closed action parser: 1 test, all passed. Regenerated both committed types with `gen-script-types`. Focused strict TypeScript compilation of the new op/readonly/mode tests and shipped menu passes with `skipLibCheck`; the aggregate SDK suite has four existing diagnostics (E10 guard fixture, unused directive, ambient `declare`, missing `Effect`), reproduced unchanged using the starting commit’s declarations/fixtures. These are recorded, not claimed as passing or changed by this feature.

- Task 6: Readiness check, independent review and fixes completed. Repair gate: 49 matched tests across 10 filters, plus the captured-warning regression, three extended existing-contract tests and two regenerated full-registry snapshot tests passed. Final preflight: `cargo fmt --check` passed; `cargo clippy --target-dir target/preflight-clippy -- -D warnings` passed; `cargo test` passed with 9,312 tests, zero failures and 42 ignored across 60 test/doc-test groups. Full tests ran with localhost socket access after sandbox denial of networking fixtures. M1–M6 remain external, blocking proof; no visual result is inferred.


- Task 7: Touched-crate check and strict focused TypeScript passed. Readiness: `cargo fmt --check` passed; 51 tests across 10 nonzero filters passed (window policy/readback/shipped activation 21, production menu contracts 1, action parser 1, TS/Luau parity 2, generated surface 1, registry snapshots 2, committed SDK drift 1, compiler export 1, options bridge 17, render extents 4). Four independent review-panel lenses approved with zero findings: correctness tracer, contract verifier and adversarial tester at Sol xhigh, hygiene/drift at Sol medium. Native M1–M6 remain outstanding. Final preflight passed: fmt and isolated clippy clean; full tests 9,313 passed, zero failed, 42 ignored across 60 result groups. Clippy initially flagged one mechanical unnecessary-lazy-evaluation cleanup; using `Option::or` for the borrowed default preserves behavior, and the completed lint/test gate passed. Only incremental build caches were cleared when disk fell below the workflow threshold; full target directories remain.

## Review checkpoint

Review-panel covered two slices and eight lens passes: two breadth passes, four correctness traces (request lifecycle, readback, boot/store, cross-slice seam), one contract verifier and one adversarial pass. Initial verdict: request changes, with one must-fix generated Luau declaration, five should-fix runtime/proof issues and four comment-drift concerns. Review workers were reused after coordinator dispatch reached its thread limit; all planned coverage completed.

Repairs were delegated under `fix-review-findings`; integrating executor retained shared integration, Cargo, plan and commits. Completed edits: Housekeeping scope includes readback, adapter zero-refresh normalization coverage, test-helper/opaque-instance/cancel comment corrections, and surviving closed Luau operation alias with directly typed module member. Both SDK snapshots regenerated with `gen-script-types`. Controller fixes now retain OS changes before Session installation and validate the picked tuple after a fresh revert. The shipped-menu integration activates actual focused controls, dispatches named reactions and reserved actions through shared production adapters, drains commands, runs the options bridge and reads live text bindings with only the native window backend injected. Focused repair gate: 49 tests across 10 nonzero filters passed; touched-crate check and strict focused TypeScript checks passed. All ten deduplicated initial review concerns were addressed. No external native result is inferred from these repairs.


Final acceptance audit found one additional concrete proof gap in A26: fake-backend reapply tests did not observe the native-only fallback warning. A focused independent verifier confirmed it. The unchanged once-per-request warning now lives in the shared controller after a failed adapter apply; the duplicate native log is removed. The existing P10 test captures one Warn for unavailable requests and zero for matching requests, including multiple subsequent frames. This preserves the Decision and lets the production policy warning be proved without a native window. Affected focused/lint checks passed. Eleven review/audit concerns were addressed in total.

## Task 6 final gate report (before owner amendment)

The full suite first ran only after the readiness, review, fixes and focused retests. Its sandboxed attempt stopped at the engine binary: eleven socket-denied networking fixtures and two existing graphics-menu/catalog-count expectations omitted the new surface. Extended those existing tests while preserving their prior assertions. A permitted retry cleared all engine tests and exposed the existing catalog wire-name assertion; extended it with explicit paths, values, defaults and read/write/persistence/replication contracts. The next run exposed two full-registry typedef fixtures that had not been regenerated with the SDK. Regenerated them through `gen-script-types --out`, verified they exactly match both shipped declarations, and passed both existing snapshot checks. These changes extend existing contracts without changing Decisions or Acceptance.

The completed continuation of the final gate passed:

| Check | Result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --target-dir target/preflight-clippy -- -D warnings` | pass |
| `cargo test` with localhost socket access | pass — 9,312 passed, 0 failed, 42 ignored |
| Focused strict TS checks for the new surface and shipped menu | pass |
| Aggregate SDK TS checks | baseline failures — four unchanged diagnostics, reproduced against starting declarations/fixtures |
| Luau static analyzer | unavailable; dual-runtime and generated declaration checks passed |
| Native macOS/Windows M1–M6 | outstanding; blocks landing |

Final full-test log: `/tmp/window-modes-preflight-tests-complete.log`; final lint log: `/tmp/window-modes-preflight-clippy-final.log`. Earlier failed-attempt logs and native sample are retained as session proof until landing cleanup. The owner requested publishing `window-modes` to `origin` for testing on both computers. This testing handoff keeps the brief test-ready; landing and durable context reconciliation await native results and the owner's landing direction.


## Task 7 current gate and testing handoff

The amended behavior passed the readiness gate (touched-crate check, fmt, 51 focused tests and strict TypeScript), an isolated four-lens review with zero findings, and the final preflight. All A1–A30 have automated proof; M1–M6 remain outstanding external proof and block landing. No native outcome is inferred. The owner previously authorized pushing this branch for testing on both computers; this amendment is committed and pushed as that testing handoff.

| Check | Current result |
|---|---|
| `cargo fmt --check` | pass |
| `cargo clippy --target-dir target/preflight-clippy -- -D warnings` | pass |
| `cargo test` with localhost socket access | pass — 9,313 passed, 0 failed, 42 ignored |
| Focused strict TS surface/menu | pass |
| Independent review-panel | approve — four lenses, zero findings |
| Native macOS/Windows M1–M6 | outstanding; blocks landing |

Current logs: `/tmp/window-modes-apply-preflight-fmt.log`, `/tmp/window-modes-apply-preflight-clippy.log`, `/tmp/window-modes-apply-preflight-tests.log`. The initial lint log and readiness logs remain available until landing cleanup. The prior aggregate TS baseline diagnostics and unavailable Luau analyzer remain as recorded in Task 6; current dual-runtime and generated-declaration guards passed.
