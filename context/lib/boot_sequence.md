# Boot Sequence

> **Read this when:** wiring startup, splash, level-load, or shutdown code; the staged level load, its threads, commit or cancellation; the first-launch accessibility hold; window visibility; or reasoning about where new init/teardown belongs.
> **Key invariant:** the engine owns the schedule; mod code runs only inside phases the engine grants. The boot path is single-window; level load/unload is repeatable at runtime, and a level load's stages run off the main thread, which pays one commit frame.
> **Related:** [Architecture Index](./index.md) · [Scripting](./scripting.md) · [Entity Model](./entity_model.md) · [Build Pipeline](./build_pipeline.md) · [Rendering Pipeline](./rendering_pipeline.md) §3 (per-load renderer sets) · [Resource Management](./resource_management.md) §8.2 (model cache) · [UI](./ui.md) §4.1 (accessibility panel) · [Player Options](./player_options.md)

Boot code lives in `crates/postretro/src/main.rs` and `crates/postretro/src/startup/`; the level load's off-main stages in `crates/postretro/src/level_load/` and `crates/postretro/src/load_pool/`.

---

## 1. Boot Order

Boot is staged so first pixels reach the window before heavy subsystems initialize. The pre-window pass resolves arguments and directories, reads player options once without side effects, assembles the boot-phase App and its pending-session owner, and creates the event loop. The options read is the only session-lifetime exception; it supplies the window mode before the first redraw and transfers into the session without rereading. The splash paints a color clear, then the logo. After the logo presents, session construction completes options persistence, initializes audio, scripting, input/UI and networking, followed by the dev-tools UI, full renderer initialization, stale-script recompilation, mod init and the hot-reload watcher. The canonical development launcher (`cargo run -p xtask -- run [cargo-run flags...] -- [postretro args...]`, or the simple `cargo run -p xtask -- run [postretro args...]` form) prepares `scripts-build` before the engine process starts; the running engine does not invoke Cargo to build that sidecar.

The renderer initializes in two phases (`rendering_pipeline.md` §7.8): **boot-ready** (surface/device/queue/surface-config + the renderer-owned boot splash pass) gates splash painting; **full-ready** (pipelines, passes, lighting/shadow/UI/fog resources) gates Frontend, a level load's dispatch and commit, Running, the UI pass, and scene rendering. Boot-ready is enough to present black and logo frames.

| # | Stage | Notes |
|---|-------|-------|
| 1 | Init logging and boot clock, collect raw args, resolve selected mod/content root + optional map path + per-user directories, read player options once | Minimal pre-window work; options read has no writes or identity generation. `--windowed` overrides the saved mode for this session without changing storage. Net role is NOT parsed here — it defers past first pixels. `--mod <name>` / `--mod=<name>` is the shipped single-mod boot handle: it names a mod, never a path, and selects `content/<name>`. A value holding `/`, `\`, or `:`, a `.`/`..` or leading-`-` value, and a `--mod` with no value are boot errors, as is the retired `--content-root`. A bare map argument is the raw-path dev bypass: with `--mod` a relative map path resolves inside `content/<name>` (`--mod dev maps/x.prl` loads `content/dev/maps/x.prl`), and one that still starts with `content/` (after any leading `./`; case-insensitively on Windows and macOS) is a boot error with a hint; an absolute path stays as given. Without `--mod` the map path is read from the working directory and the content root is derived from it. `--app-name <name>` / `--app-name=<name>` names the per-user config and data directories, resolved once here; every settings and `state.json` read and write takes them. It rejects a value holding `/`, `\`, or `:`, a leading `-`, a nameless flag, and a value that is blank, `.`, or `..` once whitespace is removed (Linux drops whitespace when naming the directory). Absent means `postretro` (`build_pipeline.md` §Distribution packaging). Every path flag — `--mod`, `--app-name`, `--baked-root`, `--core-root` — is read at its first occurrence; an empty `--flag=` is that occurrence, not a pass to a later one. |
| 2 | Create the event loop; options already loaded | No audio, scripting or net work happens here. The event loop is created, and the `PendingSessionInit` owner is set up to carry deferred inputs. All session construction (`Session::build`: preloaded options completion, scripting bootstrap + registries + `ScriptCtx`-clone systems + built-in UI registration, audio, input/UI/modal group, net endpoint) runs post-first-pixel on the logo frame. Primitive closures capture `ScriptCtx` clones; held for the whole session, never recreated. |
| 3 | Construct the app in the Booting state with a `PendingSessionInit` owner holding the raw net args | Mod init, hot-reload watcher, audio, debug UI, net endpoint, and the engine load pool (built at the first load dispatch) are NOT done here. The pending owner carries deferred net-setup inputs and the preloaded options document/path. |
| 4 | On resume: create window (**visible** before the first redraw request), apply the saved window mode (on Windows, a saved exclusive mode as a borderless interim), init the **boot-ready** renderer (surface/device/queue/config + boot splash pass), apply a deferred exclusive mode, enter Splash, request first redraw | Fires once on desktop; guarded against re-entry. The window must be visible before the first redraw is requested: the redraw-driven splash loop depends on the OS delivering `RedrawRequested`, which on Windows only happens for a visible window — a hidden window receives no `WM_PAINT`, so a "create hidden, reveal after first present" scheme hangs boot. Today it is created visible (winit default); the decided accessibility-adapter change creates it hidden and shows it immediately after the adapter exists, still before that request (see § "Window visibility (Windows white-flash)" below). Device creation already requests the FULL feature/limit set (wgpu features can't be added later). Adapter checks the boot splash needs run before the first splash draw; hard renderer requirements fail fast with a named message (`rendering_pipeline.md` §4). No audio, debug UI, net, mod, or level work in this path. |
| 5 | Redraw drives the splash state machine (below) | |
| 6 | Present the frontend or enqueue the boot map load | No CLI map → frontend menu. If the committed frontend declares a background level, the menu stays open while that catalog id loads behind it. CLI map → Loading. On first launch, the accessibility-panel hold comes first (below). |
| 7 | Loading dispatches the requested load's stages and polls them; the frame its bundle is ready commits it (§3); Settling holds it until its first frame's streamed set is resident | Parse and prep on the engine load pool, the renderer resource set on the upload thread (§2); one main-thread commit frame. Repeats for runtime load requests (§4). Every level entry settles (below). |
| 8 | Steady per-frame loop: Input → Game logic → Audio → Render → Present | Running draws the level. Frontend draws a world-less frame and frontend-safe UI. A frontend menu over a loaded backdrop runs through Running with UI capture and the menu camera pose held. |

### Splash state machine

The boot splash is a **renderer-owned** path — a direct boot splash pass that clears the swapchain and draws the decoded logo as a textured quad. The logo is centered at half the width of the largest 16:9 frame that fits the window, keeping its aspect under a height cap: half the window width at 16:9 or narrower; on a wider window, the size a 16:9 window of the same height gets. It does NOT use the UI system, UI JSON, glyphon, taffy, mod themes, the UI image registry, or `UiReadSnapshot`. The splash advances one frame per redraw, deferring all CPU-heavy work behind visible pixels. After successful command submission, the renderer returns a `PresentHandle`; only frames with a handle advance stage marks and scheduling. Transient or skipped acquire paths return no handle and re-request redraw without advancing.

- **First frame (splash-color clear).** Paint the first frame (no logo bound — a `SPLASH_CLEAR_COLOR` clear) into the visible window. After it presents: record `first_black_frame`, then decode and upload the splash image (bounded CPU work, single-threaded). A missing or malformed splash asset warns and keeps the frame at the splash-color clear — never aborts boot. No mod, session, audio, net, or level work yet.
- **Second frame (logo / fallback black).** Paint the splash so it is visible before any user-authored work; record `first_splash_frame` after present. Then, behind the now-visible pixels (or behind the fallback black frame when the asset was unavailable): install the whole `Session` via `Session::build` — preloaded options completion (seeds input; generates identity and saves only now), fault-tolerant audio, scripting bootstrap + registries + `ScriptCtx`-clone systems + input/UI/modal group, then the net endpoint; a hard script-runtime construction failure logs, stores the error, and exits boot. After session install: lazy-init the dev-tools debug UI (window-derived, separate from session install), then complete full renderer initialization, then recompile stale scripts (debug-only), run mod init, and start the hot-reload watcher (debug-only). Full renderer init runs **before** mod init because mod init installs the manifest's UI theme and font faces through full-ready renderer paths (`set_ui_theme` / `register_ui_font`, which touch `Renderer::full`) — those paths panic on the full-ready guard if `Renderer::full` is not yet built. Session install stays ahead of full renderer init: `Session::build` is CPU-side and its failure must early-return before any full-ready work runs. Mod init commits the validated entity descriptors, mod-state declarations, UI/theme data, frontend declaration, loading-screen declaration, UI image map, static render profile (bloom resolution/style), and map catalog from `ModManifest`, then loads the eager UI images, so the first Loading frame of a CLI boot map already has them (loading-only images wait for their loading screen, below); persistence overlays declared defaults only on the first successful commit. A failed mod init commits none of them, so the renderer keeps its active bloom profile. Full renderer init completes **before** splash clears and before any Frontend/Loading-completion/Running/UI/scene path runs. With no CLI map, clear splash and present the frontend menu. With a CLI map, enqueue a raw-path load request and enter Loading.
- **Loading frames.** Each polls the transport, dispatches a queued load once nothing blocks it (§4), then polls the load's stages without blocking. While they run, it paints the loading screen (below). On the frame the bundle is ready, it commits the level (§3) instead of painting and enters Settling (below). The reveal clears the splash and enters Running. A failed load takes the one failure route (§4): a runtime load enters Frontend, a CLI boot-map load exits non-zero.

The two-frame delay is causal: pixels reach the user before any deferred session, audio, net, mod-supplied, or level-load CPU work runs.

**OS preference wait.** The OS preference reader starts at session build, after the first splash frame has presented, and never blocks a frame: splash frames keep presenting while it is pending. Whatever follows mod init — the splash clear to the frontend or the first-launch panel, or the CLI boot-map enqueue — waits for its first reply up to 150 ms, counted from the end of mod init, so a slow mod init still gets the full wait. The wait gate polls the reader's channel itself before checking for a reply, because the session (and so the frame-top poll) does not exist yet on the frame mod init runs: a reply already in when mod init finishes therefore adds no frames. A later reply applies as a live change through the frame-top poll, which runs every frame in every boot state once the session exists (`player_options.md` §5).

### Loading screen

Every Loading frame — CLI boot map, catalog load, frontend backdrop, network relevel — draws a loading tree, a UI tree composed alone over a world-less frame cleared to the splash background color, so the boot splash hands off with no color step. It is display-only: never pushed onto the modal stack, no focus ring, and UI input stays dropped as on every non-UI frame. It runs its own UI clock from zero when a load begins it, so authored tweens advance without touching `script_time`. A load that supersedes one in flight (§4) keeps the tree, its clock and its images, shows the new level's name and restarts the bar at 0 on its own counter, which the superseded load's stages never credit. The renderer-owned splash paint remains the fallback for a Loading frame with no session, no full-ready renderer, or no registered loading tree.

- **Choice.** Once per load, when it begins: the catalog entry's `loadingTree` pool, else the mod's `loading.tree` pool, else the registry name `loadingScreen` — an engine fallback (`core/ui/loadingScreen.json`: the splash logo where the splash draws it, a bar beneath) a mod shadows by registering the same name. A pool picks uniformly among its names registered at engine or mod tier; an unregistered name warns once. The level tier is never a candidate: the outgoing level's trees are gone before Loading begins. Raw-path loads have no catalog entry and start at the mod pool.
- **State.** `loading.levelName` (catalog name, else file stem) and `loading.progress` are set when the load begins and reset when it ends: at reveal on success, or on failure. Progress reads one phase-weighted counter for the whole load, owned by the level-loader: parse 0.40, prep 0.15, upload 0.15, settling 0.30. Each phase credits real units of its own work from the thread doing it (parse: bytes read; prep: tasks finished; upload: set-build parts finished; settling: the settle set's shortfall). A phase with no work credits its whole share on entry, so a near-empty level still fills the bar. The fraction never decreases, stays below 1.0 until every phase finishes, and reads exactly 1.0 on the frame that decides the reveal. No crate above the level-loader keeps a counter of its own. A load beside a running level (§4) credits only its own counter; nothing reads it into `loading.progress`.
- **Images.** The chosen tree's loading-only mod images (`ui.md` §5) start decoding on the engine load pool (§2) when the load chooses its tree, ahead of any queued level prep; one already registered is skipped. Loading and Settling frames poll without waiting and register each image as it arrives, so the background is absent until its decode lands while the tree's widgets draw normally; on a short load it may never appear. Every end of the loading screen — reveal, failure, abandon, network relevel, a load that replaces it, suspend — unregisters what that load registered and discards decodes still in flight. An image the installed level's trees name is the exception: install promotes it to eager (`ui.md` §5), and the load no longer releases it. A decode that arrives after a staged reload changed or reclassified its entry is discarded. Each load re-decodes its images.
- **Commit frame.** A load's stages run off the main thread (§2); the main thread pays one commit frame (§3), which paints nothing, and the bar animates through every other Loading and Settling frame. A runtime load in flight never holds requests back: request draining runs on every Loading frame, and a CLI boot load refuses runtime requests instead (§4).

### Settling

Every level entry — boot map, catalog load, `restartLevel`, frontend backdrop, network relevel — passes from install into Settling, never straight to Running. The level is installed but not shown until every streamed resource its first frame draws is resident, or a timeout runs out. The goal is the first complete frame, not the earliest one. The sim must not tick: nothing may act on the player while they cannot see.

| Runs during Settling | Waits for reveal |
|---|---|
| Visibility and the level's streaming drains from the presented pose, over a compose-only frame with no world draw | Sim tick, game time, timed-reaction frame counter |
| World-less net poll, so a long hold drops no peer | World present |
| Staged-manifest poll | Level audio: sounds install queued dispatch on the reveal frame |
| Level-request drain | UI input (dropped, as on every non-UI frame) and gameplay input (suspended each frame) |
| | System-command drain, world net poll |

Settling counts as installed: request draining, parity, script hot-reload recompose and observe-live treat it as they treat Running.

- **Loading tree.** The loading screen stays active, so its tree keeps drawing, never the bare splash, and the flash limiter ages across the stretch as it does across Loading. `loading.progress` keeps rising through Settling's share (§1 State) as the settle set's shortfall shrinks. A shortfall that grows stalls the bar instead of moving it back. The last Settling frame paints 1.0, timeout included.
- **Settle chokepoint.** One check decides whether Settling may end. It asks each streamed resource whether the presented pose's settle set is usable: SH clusters Sampleable, lightmap blocks installed. A resource that does not stream answers settled. A streamed one that has not yet derived demand from a view does not. The settle set is what the first frame draws plus the lead: SH's mandatory tier, and lightmap's mandatory blocks plus every block the pose draws on any visibility path, the planar mirror's included (`rendering_pipeline.md` §4). Optional targets never hold the reveal. An SH refusal, at a device limit or the whole-map cap, remains an unsettled mandatory target until timeout; it does not end the session or release Settling early. Capture still requires a sampleable settle set. Settling drains at a larger per-drain byte budget than play, since no world frame is at stake.
- **Timeout.** 10 s, started fresh on each entry. A timed-out release logs one warning naming each resource's unsettled count, then reveals with each resource's miss fallback. A settle on the deadline frame is a settle and warns nothing. A timed-out reveal is a reveal for every purpose, co-op included.
- **Entry log.** Each revealed entry logs one `[Settling] entry` info line at the end of its reveal frame: outcome (settled or timed out), hold time, Settling frames, how many took the portal walk and how many the step-limit fallback, peaks of the settle set and its unsettled count per streamed resource (SH targets, lightmap blocks), and the reveal frame's lightmap misses. Three planar fields follow (`rendering_pipeline.md` §7.3a): `planar_held` and `planar_reveal`, the height of the plane the mirror drew on the held frames and on the reveal frame (`none` with no mirror view); `mirror_only_blocks`, lightmap blocks of mirror-drawn cells the main view does not draw, outside the camera cell's baked set and the pins. A resource that does not stream reports `n/a`. An abandoned entry logs none.
- **Abandon.** A load or unload request during Settling takes the ordinary unload path, and suspend resets it. Either drops the settle state, its timer and the commands install queued, and fires no reveal edge. No release is decided on a frame whose net poll queued a request.
- **Reveal edge.** A Settling frame decides the release and paints the bar at 1.0. The next redraw, after its request drain, reveals and runs as the first Running frame:
  - Input that arrived since the decision is suspended, and the reveal frame applies no look, so it presents the settled pose.
  - The fixed-step accumulator re-arms: the sim owes nothing for the hold, and the first tick usually lands on the next frame.
  - The loading screen ends and the splash clears.
  - A baked light promoted on the reveal frame starts at full weight; an entry has nothing to fade from. Later promotions crossfade.
  - The net reveal is published (`networking.md` §Slot lifecycle).
  - Line C closes `settle_hold` on the deciding frame and `reveal` at the edge, and records `first_level_frame` after the reveal frame renders.
- **Presented pose.** One chokepoint resolves the position, yaw and pitch the first Running frame presents. Whenever the frontend menu is present it is the menu pose. That covers a backdrop install and a co-op client following a relevel with the menu pushed, which install cannot tell apart. Otherwise it is the spawn pose: the followed pawn's eye, else the start pose. Install places the camera there, Settling streams from it, and the reveal frame presents it.

The gate applies only at level entry. In play nothing waits on residency: a miss takes its resource's fallback on the frame it happens.

### First-launch hold

With no stored record that the accessibility panel was shown, boot shows the panel (`ui.md` §4.1) once mod init completes and the splash clears — before any frontend backdrop, CLI boot-map, or host-named level load starts. On the CLI path the splash clears to a world-less panel frame; Loading repaints after the panel closes. No level runs and no sound plays until the player closes it; boot then presents the frontend or loads the boot map as on any launch. The hold is its own boot state: it drains no level requests, keeps the world-less transport alive, and runs the options bridge's settled save, so a panel change made at the hold persists before the panel closes. Hold frames take the frontend's world-less frame path with the HUD hidden. Closing the panel by any path ends the hold through the same boot-destination step the splash clear uses.

Opening the panel first is not enough, because a capturing modal pauses neither simulation nor audio (`ui.md` §4): a panel over a loading level would expose the player to what the panel exists to prevent.

- **Close paths.** Cancel or the panel's close button; no reserved input toggles it (`input.md` §5). UI input pressed during the splash is dropped, so a buffered cancel cannot close the panel on its first frame.
- **Record.** Written when the player closes the panel by any close path. A launch that quits or crashes with the panel open shows it again. A settings file that cannot be replaced (unreadable, or not valid TOML) cannot take the record, so the panel shows each launch until it can — the fail-safe. Later launches boot normally.
- **`--connect` client.** Stays admitted, not participating, through the hold (`networking.md` §Admission and content parity). A map its host names during the hold — or during the splash's OS-preference wait on any launch — is queued, not loaded, and is the level it loads when the hold ends or the splash clears; neither the frontend backdrop nor a CLI boot map displaces it. Hold exit and splash clear share one boot-destination step that resolves this priority. When the host's map replaces a CLI boot map, that step pushes the frontend menu in the same tick, before dispatching the load as an ordinary runtime request. The menu is not a visible interstitial, since Loading paints the splash. It is the fallback a failed or unresolved host load returns to, because the hold leaves the modal stack empty.

### Window visibility (Windows white-flash)

**Rule: the window is visible before the first redraw is requested.** Today it is created **visible** (winit default). On Windows the OS paints a freshly-created window with its class background brush (white) before the app's first wgpu frame is on screen, so boot shows a brief pre-first-present white flash, then the splash-color frame. This is an accepted cosmetic artifact.

A "create the window hidden (`with_visible(false)`), then `set_visible(true)` after the first present" scheme was tried to suppress the flash and **hung boot on Windows** — it was reverted. The cause is a winit-0.30/Win32 interaction confirmed in the winit source: `Window::request_redraw()` calls `RedrawWindow(hwnd, NULL, NULL, RDW_INTERNALPAINT)` (`platform_impl/windows/window.rs`), and Windows does not deliver `WM_PAINT` (hence no `RedrawRequested`) to an **invisible** window. The splash state machine is driven entirely by `RedrawRequested` under the default `ControlFlow::Wait` (no `set_control_flow` anywhere; the event loop blocks in `MsgWaitForMultipleObjectsEx(.., INFINITE, ..)`), and `about_to_wait`'s `request_redraw()` produces no paint for a hidden window either. So with a hidden window, frame 0 never receives its redraw → `run_splash_frame_zero` never runs → the window is never revealed → permanent hang (symptom: `[Engine] Window ready` then silence).

A proper flash fix must not gate the first frame on an OS paint event delivered to a hidden window. Viable directions (not implemented): register the window class with a background brush matching `SPLASH_CLEAR_COLOR` so the pre-first-present paint is the splash color rather than white (Win32, would need platform-specific window creation); or drive the very first frame proactively (render + present while hidden, then `set_visible(true)`) **without** depending on `RedrawRequested` — but DXGI present-to-hidden-window behavior (whether it returns `Occluded`) is not guaranteed across drivers, so this was not adopted. A booting engine with a brief cosmetic flash is strictly better than a hang.

**Accessibility adapter (decided, not yet built).** The screen-reader adapter (`ui.md` §4.2) must be constructed against a not-yet-visible window. Boot therefore creates the window hidden, builds the adapter, and shows the window immediately — before the first redraw is requested. This differs from the reverted scheme, which waited for a first present that a hidden window never produces; here nothing waits on `RedrawRequested` while the window is hidden. It is unproven until manual Windows boot reaches the frontend without hanging for every saved window mode; repeat the window-mode boot matrix when integrating this adapter.

**Window mode.** Player options load once at stage 1, before the window, and ride the pending-session owner into `Session::build` — the deliberate pre-window exception: the read is side-effect free, `Session` stays the sole owner, and device-identity generation and the first-launch save stay post-first-pixel. A saved fullscreen mode applies immediately after the window is visible and before the first redraw request, never as a creation attribute on a hidden window: winit leaves hidden + fullscreen unguarded on macOS. On Windows a saved exclusive mode shows borderless until the boot-ready renderer has created its surface, then switches to exclusive. Observed on DX12, root cause unconfirmed: wgpu's default HWND swapchain creation fails once the window holds an exclusive display mode, while an existing swapchain resizes through the change as on a live switch. wgpu's DirectComposition swapchain avoids the failure but changes every frame's present path, so the boot-only interim is preferred. The gate is the platform, not the backend: the adapter is unknown until the renderer picks one, and wgpu falls back to DX12 wherever Vulkan is missing. `--windowed`, parsed at stage 1, overrides it for the session (`player_options.md` §7).

### Deferred-session boundary and single commit

Session-dependent systems (script runtime/context, registries, completed options ownership, input, modal stack/UI registrations, frontend/focus state, net endpoint) are grouped so boot paths before install touch only pending inputs plus boot window/renderer state. The pending session owner is consumed exactly once (take-on-install), so a suspend/resume re-entering the splash loop never re-runs deferred init. The redraw-path stale-script reload drain is gated behind that install — it never runs before the script runtime exists and the first logo frame paints.

### Session boundary (type-enforced)

All session-lifetime state — the scripting core (context, runtime, primitive/reaction/system/classname registries, every system holding a `ScriptCtx` clone or registry reference), the input/UI/modal group, player options + settings path, the committed frontend declaration, the net endpoint, audio, and (dev-tools) the debug UI — is owned by one `Session`, held as `App.session: Option<Session>`. Boot-lifetime fields (renderer, window, camera, boot timings, the level-load state machine, the engine load pool, the stage retirement list, the pending-session owner) stay on `App`.

The boundary is **type-enforced**: while `App.session` is `None` (Booting/Splash before install), boot-phase code physically cannot name a session field. It may borrow the preloaded options from the pending owner to apply the saved window mode. There is no `session_mut()` accessor; handlers reach session state through a scoped `self.session.as_ref()/as_mut()` borrow disjoint from the renderer/window field paths.

`Session::build` is the **sole** construction site: whole-or-nothing, synchronous (no `await`, no yield), post-first-pixel, run inside the single install redraw on the logo frame. It builds in boot order: preloaded options completion (seeds input; generates identity and saves only now), fault-tolerant audio, the scripting bootstrap + registries + `ScriptCtx`-clone systems + input/UI/modal group, then the net endpoint. The only fatal step is script-runtime construction; audio, net, and built-in UI-tree disk loads degrade in place.

**Install / failure flow.** The pending-session owner is consumed once via `take_once(pending_session)` (single-commit guard). On success the built `Session` is stored and boot proceeds. On a `Session::build` `Err` the install stores the error in `exit_result`, requests `event_loop.exit()`, and early-returns from the install frame so no later step runs against a `None` session — mirroring the renderer full-init failure path. The build-result → action decision is a pure classifier, tested without a window or GPU.

**Suspend/resume.** Suspend keeps an installed `Session` (and an unconsumed pending owner) alive; the single-commit `take_once` guard means a resume re-entering the splash loop never re-runs deferred init. Only window-derived session state (the dev-tools debug UI) is dropped on suspend and lazily rebuilt on the resumed logo frame.

### Startup timing vocabulary

`StartupTimings` records ordered named stage marks (logged as `[Startup] …`). Boot marks in order: `args_parsed`, `event_loop_created`, `window_created`, `wgpu_init` (boot-ready renderer), `first_black_frame`, `splash_decoded`, `splash_uploaded`, `first_splash_frame`, `audio_init_complete`, `script_runtime_ctor` (post-first-pixel, inside `Session::build`), `net_endpoint_complete`, `os_reader_started`, `session_init_complete`, `renderer_full_init_complete`, and (CLI-map boot) `boot_worker_dispatch`, which records the boot load request's enqueue. `first_black_frame` / `first_splash_frame` are recorded only after the renderer returns a `PresentHandle`; the window is visible before the first redraw request (see "Window visibility (Windows white-flash)"). `first_black_frame` precedes the net/audio/debug-UI/mod marks, the boot load's enqueue **and** `script_runtime_ctor` — the script runtime is built behind first pixels in `Session::build`.

Level-load line C (`[Startup] …`) top-level stages are disjoint, consecutive and add up to the load, from its start to the reveal frame: `load_dispatch`, `bundle_ready`, `commit_frame`, `settle_hold`, `reveal`, `first_level_frame`. Two kinds of mark print in parentheses after a top-level stage (`stage=… (incl. …)`) rather than as stages of their own:

- **Measured on another thread, inside the stage.** `bundle_ready` (dispatch to the frame the bundle is taken) carries `prl_parse`, `upload` (first set-build part submitted to the set's receipt) and one mark per prep kind; a kind that runs several tasks prints as `<kind>_x<n>`, their sum. These overlap one another and the stage, so they do not add up.
- **Folded main-thread marks.** `commit_frame` folds the commit's own marks, which do sum to it, in §3 order: `commit_checks`, `set_publish`, the segment-B marks (`bridges_populated`, `classname_dispatch`, `data_script`, `archetype_sweep`), `model_install` (prepared clip entries and hit zones, the counted miss fallback included), `model_bindings` (clip-index resolve and zone-multiplier check), `progress_membership`, `level_load_event`, `host_registration` (also holds the bindings that follow segment B), `camera_pose`, `fog_masks`, `sprite_collections`, `audio_load` (registering the decoded level sounds), `streaming_sessions`, `enter_settling`.

`settle_hold` closes on the frame that decides the release, and `reveal` at the reveal edge, a timed-out reveal included. A load beside a running level (§4) logs `load_dispatch, bundle_ready, bundle_held, commit_checks, running_release, commit_frame, settle_hold, reveal, first_level_frame`: `bundle_held` is how long the ready bundle waited for its caller, its checks run before the running level's release, and `commit_frame` then carries no `commit_checks`.

Marks that `StartupTimings` cannot carry log their own lines. Every set build logs `[Renderer] Geometry install timing:` with its geometry phases (buffers and SH streaming init, SDF atlas and compose family, lightmap and SDF shadow, BVH and cull, `other`); for a load it logs from the upload thread. The renderer never sees app-side timing types. Unload logs its own `[Startup] unload_level=` line: in the loading-screen order it runs before the next load resets the level timings, and beside a running level it falls inside `running_release`.

---

## 2. Load Threads vs. Main Thread

A level load is one staged pipeline: a staged parse, prep tasks on the engine load pool, the renderer resource set built on the upload thread, then one commit frame on the main thread (§3). Every stage before the commit runs off the main thread, behind the loading tree or beside a running level (§4), and nothing a stage creates reaches the frame path before commit.

| Owner | Work |
|-------|------|
| Main thread | winit event loop, the frame's wgpu work (per-frame uploads, render, present), audio mixer, all script-VM execution, the load state machine (§4) and each load's commit frame. The script runtime and renderer are not `Send` — enforced by the types. |
| Engine load pool | The staged parse and every prep task: world-texture prep (`.prm` read and parse, slot plan), the indirect index-range check, the SH residency plan, level assembly and UV normalize, static collision, the streaming sessions' level-data checks, the lightmap-family plan, the fluid pack, sound decode, model prediction and glTF parse with clip and hit-zone tables. Loading-screen image decodes too. Output is plain `Send` data — no engine handles, no GPU objects. |
| Renderer upload thread | The install pipelines, once per renderer, then each load's renderer resource set (`rendering_pipeline.md` §3). The only off-main code that creates GPU objects. |
| Streaming threads | In-play SH and lightmap reads (`rendering_pipeline.md` §4). |

Load stages start no threads beyond these; no async runtime.

**Staged parse.** The parse hands out five ordered groups as their sections decode: the SH stream manifest's metadata (no cluster topology); geometry with texture names and cache keys; spatial data; entities; then a final group holding the lights and every lighting section, each section a parse-end step rewrites, the SH cluster topology, and late validation's verdict. Each prep task starts as soon as the groups it reads exist, so texture prep runs before the parse's last section read and the set build before the parse returns. Late validation (cross-section checks, the cluster directory) may reject after earlier groups went out: a task's failure before the verdict waits for it, a rejection replaces that failure, and the final group goes out only on an accepting verdict. Cancellation is a cooperative check between parse stages, never inside a positional read.

**Prep checks.** Every check that can fail on the level's own data runs in prep: the index-range check, the static collision build, the streamed-SH residency plan, the id-49 hint decode and the other streaming level-data checks. A failing one is an ordinary load failure, so the commit never fails on level data.

**Engine load pool.** One pool, engine-owned, built at the first load dispatch and kept for the boot, suspend included; exit shuts it down without waiting on a running task. Its width has a low, named cap, because beside a running level prep shares the CPU with that level; tests run it at width ≥ 2. Each load's tasks form one cancellable group and finish out of order. Loading-screen image decodes run ahead of queued level prep. A cancelled group drops its queued tasks, flags its running ones, and retires by poll-and-join on later frames, never joined on the event-loop thread. A task never waits on another task's result: at width 1 that deadlocks.

**Upload thread.** Renderer-owned, started at the end of full init; its first job builds the install pipelines. A load's set builds there from renderer inputs snapshotted at dispatch and touches only resources not yet published. Every job runs under error scopes, so a wgpu validation error or a panic in it becomes the load's failure. Its writes submit in batches of at most 32 MiB, and each job submits its last write before delivering its result, so a set's writes are submitted before any frame draws it (`rendering_pipeline.md` §1).

**Textures** decode from baked `.prm` mip sidecars, not from the PRL. Missing or unusable sidecars degrade per-texture to placeholders with a warning — not a load failure.

**Progress.** Each stage credits its phase of the load's counter (§1 State) from the thread doing the work; the main thread reads the combined fraction each Loading and Settling frame. Parse units are the bytes this load actually reads: sections read whole, plus the file image itself on the whole-file backing. Streamed SH bodies and lightmap blocks past their index prefix are not read at load and do not count. The parse plans its total after reading the section table, so its share moves before the first section read.

---

## 3. Level Install Order

A level installs in one main-thread commit frame: the Loading frame its bundle is ready, or, beside a running level, its caller's commit (§4). The commit holds only main-pinned work: resource hand-back and the script-VM and registry stages, in the order below. Everything else ran off the main thread first (§2): the range check and collision build, texture prep, UV normalize, the renderer set build at the tiers the load snapshotted at dispatch, sound decode, model parse. Texture prep precedes UV normalize: `.prm` slot dimensions drive it, so texel-space UVs convert to `[0,1]` only once those dimensions exist. The commit never waits on a stage thread or the GPU. In the loading-screen order every commit follows the unload of any active level.

**The commit cannot fail on level data.** Prep ran every level-data check (§2). The commit's own checks (stage 1) need state only the commit sees, run before anything mutates, and fail through the one failure route (§4). Past them nothing returns an error.

**Counted fallbacks.** The commit creates no pipeline, texture, buffer or bind group, and parses or decodes nothing, outside three counted fallbacks: a model the prediction missed (stage 12), a scene-extent rebuild (the set's extent-bound groups rebuilt once at publish when the scene extent or targets moved since its build), and a reflection-tier rebuild (the set's reflection targets re-keyed to the live tier once, when the tier changed on a Running frame while the load ran beside that level). Sprite-collection registration (stage 15) is a named exception: its candidates come from the post-spawn registry. Counts come from the renderer's one creation chokepoint; a creation outside them logs a warning.

| Order | Stage |
|-------|-------|
| 1 | Commit checks: the bundle's set still resident and unpublished on a full-ready renderer, and the level's streaming sessions built against that set's residency snapshot. A failure releases the set with nothing mutated and no parity published |
| 2 | Identity: the environment volume set and its query tree, replaced whole, then the level tags, the join seed, level parity and the relevel id (`networking.md` §Admission and content parity) |
| 3 | Seed the level default gravity from the EnvironmentVolumes header (before scripts run); build navigation |
| 4 | Publish the load's set, retiring the active one: textures, normalized geometry, lighting families, fluid surfaces, fog target and cell masks, kinematic geometry, its models (`rendering_pipeline.md` §3). Publish merges the set's samplers into the renderer's add-only maps and runs the extent and reflection-tier fallbacks; the set's new model entries then enter the shared cache (`resource_management.md` §8.2). Fluid surfaces draw still until drift resolves (note below). Then the planar verdict and the light seed |
| 5 | World mesh spawn (see seam note below) |
| 6 | Light bridge: one light entity per map-authored light |
| 7 | Collision world committed from static geometry (built in prep, separate from BSP; a rejected mesh — a defensive route, see `entity_model.md` §7 — fails the load before commit). It exists before the first tick, on the renderer-less path too |
| 8 | Fog bridge: fog-volume entities; the fog target at the level's pixel scale and its cell masks were built into the set |
| 9 | Built-in classname dispatch (player spawns partitioned out; remainder dispatched, handled set stashed) |
| 10 | Data script run → compose active reactions/crossings from matching mod-global definitions plus level-local definitions; rebuild reaction subscribers, then `TriggerBindingTable::build` from the final active reactions |
| 11 | Data-archetype sweep (match map placements against registered entity types not already handled), player spawn, camera placed at the presented pose (§1 Settling): the first player spawn (or geometry center), or the menu pose while the frontend menu is present |
| 12 | Mesh model sweep: install each prepared model's clip entry and hit zones (a model the prediction missed loads synchronously, counted; one it over-predicted leaves the set, counted, creating nothing), then resolve every animated mesh entity's clip indices (see mesh-sweep note below) |
| 13 | Capture kill-progress membership (host and single player only): every map-placed entity's tags, now that placements, pawns and spawners exist and before anything `levelLoad` spawns |
| 14 | Fire the `levelLoad` named event |
| 15 | Sprite-collection registration — a single windowed pass registering every distinct emitter `sprite` in the registry (map- and descriptor-spawned alike) plus the weapon-impact collection; folded from two interleaved passes, runs after `levelLoad` once segment B has populated the full registry (see enrollment note below) |
| 16 | Level sound registration from the sounds prep decoded off `sounds/`, in sorted path order whatever order they decoded in (fault-tolerant; silent if audio init failed) — after `levelLoad` (see note below) |
| 17 | Free the model-cache entries nothing references (one sweep, after the model install), promote loading-only images a level tree names (§1 Loading screen), install the streaming sessions stage 1 built (SH, lightmap), reading nothing, then enter Settling (§1) |

Lights come from PRL data via the light bridge, not classname dispatch. Entity types and mod-global reaction/crossing definitions arrive at mod-init via the mod manifest. `setupLevel`/the data script contributes level-local reactions and crossings only (see `scripting.md` §2). Level install composes active behavior after the data script returns and before crossing initialization and progress subscription, so both see the final active set. Progress membership is captured later (stage 13), so it counts the map-placed NPCs the archetype sweep spawns.

**No streamed read at install.** Neither the set build nor the commit reads a streamed SH cluster or lightmap block; Settling's drains make the presented pose's set resident before the first world frame. Capture and tests keep a synchronous lightmap preload of their own.

**Fluid surface drift.** Drift first resolves after the `levelLoad` fire (stage 14), once the boot pawn exists, so the host reads its own descriptor's terminal velocity. Every later fluid-table or tuning refresh re-resolves it (`rendering_pipeline.md` §7.3a).

**Mesh-spawn seam.** World mesh spawning is its own install stage, distinct from classname dispatch. The durable contract: map geometry becomes renderable mesh entities here, after the set holding the level's geometry publishes and before the light/fog bridges. (The current implementation hardwires a single world mesh; a classname-driven handler is planned — see §9.)

**Mesh-sweep order.** The single mesh model sweep (model install + clip-index resolve) runs *after* the data-archetype sweep (stage 11). Its upload set unions live mesh entities, every movement descriptor, weapon presentation models, resolved spawner archetypes, and client-suppressed enemies. Runtime materialization never uploads models. Newly accepted or spawned meshes resolve clips and socket bindings against the install-time tables before presentation. The sweep runs *before* the `levelLoad` fire (stage 14), because a `setAnimationState` reaction in `levelLoad` requires resolved clip indices. Prep predicts that upload set from the dispatch snapshot (map entities, descriptors including a pending reload's deferred ones, the connected-client role) and parses each predicted glTF once on the pool, building its clip entry, hit zones and material CPU products from that one parse; the set build uploads it, or takes the shared cache's entry for the same content (`resource_management.md` §8.2). The commit's own sweep stays the source of truth, so correctness never rests on the prediction: a model it needs that prep missed loads synchronously, counted and logged once; a predicted model it does not use leaves the set. A model that fails to load warns, and the level loads.

**`levelLoad` vs. dynamic light/sprite enrollment.** The `levelLoad` fire (stage 14) precedes two windowed-only passes that run after this CPU-install sequence completes: absorbing untracked `LightComponent`s into the light bridge, and the single sprite-collection registration pass (stage 15). A `levelLoad` reaction that spawns a dynamic light or emitter is picked up by these passes and renders.

**Level-sound registration position.** Level sound registration (stage 16) runs after `levelLoad`, not alongside the earlier bridges. Safe because `playSound` enqueues an async `SystemReactionCommand` drained a frame later, after the commit completes — no reaction observes unregistered sounds during the commit.

---

## 4. Runtime Load/Unload

Runtime level requests drain at the redraw boundary before gameplay/world work for that frame, on Loading, Frontend, Settling and Running frames. Splash (the OS-preference wait included) and the first-launch hold drain none, so a request they queue starts no stage work until boot leaves them.
Streamed-SH reload cancellation never joins a live positional read on that boundary. The old
four-worker pool retires cooperatively; the next pool starts only after all old handles join,
so generations never overlap and process/session teardown still releases the retained file
after worker join.

| Request | State behavior |
|---------|----------------|
| Load from Frontend | Start a load and enter Loading; its bundle commits (§3), enters Settling, reveals into Running. |
| Load from Running or Settling | Unload current level first, then start the load and enter Loading. A Settling level unloads without a reveal. |
| Load during a runtime load | Supersede it: the old load is cancelled and reports nothing; the loading screen carries over (§1). |
| Return to frontend | Clear per-level state, enter Frontend; if the frontend declares a background level, enqueue that catalog id so it loads behind the menu. |
| Unload from Running or Settling | Clear per-level state, enter Frontend. |
| Unload during a runtime load | Cancel the load and enter Frontend, ending its loading screen. A backdrop arrives only through a load the same return already queued. |
| Any runtime request during a CLI boot load | Refused at enqueue with a warning; the boot load is not cancelled. |
| Failed load | The one failure route (below). |

### Load state machine

At most one load is active, under a generation number that increases with every load. Its phases:

- **Queued.** Resolved, its loading screen up, no stage work at all. It dispatches once a full-ready renderer and the session exist and no older load's stages are still retiring, so no two generations' stages overlap and no set build starts on a renderer that is not full-ready.
- **Staged.** Dispatched: prep on the pool, the set build on the upload thread (§2). Dispatch snapshots the player tiers (shadow, Surface Depth, reflection), and every stage reads that snapshot. Each frame polls the stages without blocking.
- **Ready.** Beside a running level only: the bundle waits, its set resident and unpublished, for its caller to commit it.

A load ends exactly once: committed, failed through the one failure route, or cancelled. A superseding request, a queued unload request or suspend cancels a load that has not committed. Requests drain before the Loading step, so one already queued when its frame finds the bundle ready cancels the load; one the same frame's transport poll receives drains on the next frame, after the commit. A cancelled load reports nothing — its stages' errors, the cancel included, never reach the failure path — and releases any set it received. Its stages, and the upload thread of a renderer dropped at suspend, join through one App-owned retirement list polled at the top of every redraw in every boot state, Booting and Splash included, never on the event-loop thread.

**Settings and extent, dispatch to commit.** A tier changed on the frame a request is queued reaches its level. A tier change after dispatch never reaches the bundle, and the live Surface Depth rewrite touches only installed materials. The two exceptions are the commit's counted fallbacks (§3): a scene-extent change rebuilds the set's extent-bound groups once, and a reflection-tier change on a Running frame beside the level re-keys the set's reflection targets once. A tier change on the commit frame applies after the commit through the live path; the shadow tier waits for the next level load. A resize recorded after the last Loading paint is the first level frame's ordinary resize.

### Commit orders

One pipeline serves two commit orders. Its stages never assume the previous level is gone.

- **Loading-screen order.** Unload any installed level, run the load behind the loading tree, commit on the Loading frame its bundle is ready. Every production load takes this order: CLI boot map, runtime requests, relevel follow, restart, the frontend backdrop.
- **Beside a running level.** No unload, no loading screen, no `loading.progress` credit. Every Running frame polls the load before its input and game logic, and a ready bundle is held. The caller's commit runs the commit checks (§3 stage 1) while the running level still runs, so a failing check keeps it Running. Then it releases the running level, which drops only that level's set and frees no cache entry, and commits the bundle in the same frame, entering Settling. The release frees nothing the bundle holds and cancels nothing. Any level request that resolves cancels a beside load first. This order is a test-only seam: no production verb starts it (§9).

### Load failure

A load reports at most one failure, through one route, whatever failed: the parse (late validation included), a prep task, the set build, an uncaptured wgpu error, a commit check, or a dispatch that could not start the pool. Every stage stops and retires, nothing commits, no registry mutates and no parity publishes; a failed or cancelled load leaves the model cache as it found it.

- **Loading-screen order.** A runtime load enters Frontend; a CLI boot load exits non-zero. The `[Loader]` error line names the level (catalog id and path, or the path), the reason, and why the player lands on Frontend: the loading-screen order unloaded the previous level before the load started, so none is left to return to.
- **Beside a running level.** The error line says the running level keeps running, and it does, with its cache entries intact.

**Uncaptured wgpu errors.** The upload thread's error scopes turn an error in its jobs into the load's failure in both orders. While a dispatched load's stages run in the loading-screen order and no level is installed, the renderer's uncaptured-error handler is armed for that load: an error first surfacing on the main thread (at a submit, say) becomes that load's failure instead of wgpu's default panic. Everywhere else the handler keeps wgpu's default: outside a load, for a queued load or one whose cancelled stages still retire, and beside a running level, where such an error cannot be told from the running level's own. Device loss is out of scope.

### Level sources and unload

`LevelSource::Catalog(id)` resolves against the engine-global `DataRegistry.maps` snapshot before the load starts. A found entry contributes `content_root.join(entry.path)` to the load and stores `{ catalog_id, path, name, tags }` and the entry's loading-tree pool on the in-flight load so catalog metadata is available before the data script runs; omitted catalog tags are stored as `[]`. A missing id logs a diagnostic and no-ops; it must not unload an active level. Raw path loads (`LevelSource::Path`, including the CLI map path and dev tooling) bypass the catalog and synthesize non-catalog metadata: no catalog id, `tags = []`, and `name` from the file stem.

Clear-on-unload contract:

| Cleared on unload | Kept across unload |
|---|---|
| Level world | Renderer device/queue, window |
| The level's renderer resource set (`rendering_pipeline.md` §3): textures, geometry and lighting families, fog target, fluid surfaces and reflection targets, kinematic geometry, smoke collections' level part, its model references, mesh-pass caches. Release drops that one set and creates nothing | Shared renderer state: install pipelines, the add-only sampler maps, the shared model cache (entries freed only by a later commit, `resource_management.md` §8.2), and a shadow tier's spot pool and depth caches while another resident set uses that tier |
| Light bridge, fog bridge, collision world | Script runtime and script context; slot table |
| Level sounds, sprite collections | Entity-type registry and mod map catalog |
| Active per-level reactions/crossings and accumulator bindings, level-local reaction/crossing definitions, UI registrations, and presentation cells | Mod-global reaction/crossing definitions and persisted-state save path |
| Progress tracker, frame-end death-event carryover, active wieldable, camera pose | Rust-side primitive/classname registries |

Frontend is a no-level steady state. Renderer and audio may exist, but world, collision, fog, level sounds, and per-level registries are empty. Frontend rendering uses a world-less clear and skips gameplay/HUD reads that require a level.

Frontend is **not** a peerless state. A net endpoint outlives every level, so a host can sit in Frontend with clients connected — the ordinary condition between maps, not an edge case — and the transport is polled from world-less frames to keep it alive (`networking.md` §Transport contract). Unloading clears the host's installed level parity, which demotes those peers rather than leaving them participating against a torn-down world.

The mod frontend hub is the player-facing menu flow. A committed `frontend` manifest block names the menu tree, required static menu camera pose, and optional background level catalog id. With no background level, the menu sits over the world-less Frontend state. With one, the menu is pushed first, then the background catalog id loads as the only active level; UI capture suppresses gameplay controls and the menu camera pose is held every frame. The root may push title-owned submenus without releasing that hold or re-enabling HUD/gameplay presentation. Back/cancel pops only the submenu; Start/menu input cannot disturb an already-open frontend modal. If the named menu tree is missing, the engine fallback frontend menu is pushed before any backdrop load starts. Starting a catalog map clears pushed menus, unloads any backdrop, and loads the selected map. `returnToFrontend()` and `ui.quitToMenu` share the return path: present the frontend menu, unload active level, then reload the declared backdrop if present.

## 5. Shutdown

- A close request or Escape exits the event loop.
- On clean exit, teardown first flushes any pending `PlayerOptions` debounce, then saves persistent mod-state slots best-effort, releases level sounds (mirrors texture release on unload), shuts the engine load pool down without waiting on a running task, and drops renderer and window.
- Abnormal termination may lose writes made since the last successful save.
- `App` holds `App.session` (which owns `script_ctx`, the scripting registries, audio, and all session-lifetime state) until the event loop returns and the process ends.

Platform suspend is a separate path. First it cancels the load in flight on the renderer that holds it (§4): a set the load received is released, a ready bundle drops uncommitted with every model entry it would have offered, and its stages retire through the retirement list. Then it hands the upload thread to that list, so a job still running on the old device is joined by a later redraw poll, and drops the renderer, its model cache with it. The resumed renderer builds its own install pipelines, and a cancelled CLI boot load reports nothing, so boot loads its map again after resume instead of exiting. Suspend also clears window/fog/collision and any loading screen (releasing its loading-only images and discarding their decodes), forgets uploaded UI images and glyph art so the resumed renderer re-uploads them, and resets state to Booting so resume rebuilds the surface and re-drives the splash loop from frame 0. It does NOT clear the installed `Session` (which owns `script_ctx`, the scripting registries, audio, etc.), an unconsumed pending-session owner, or the engine load pool. The single-commit guards keep deferred session init from re-running and let renderer full-init rerun idempotently. Suspend during the black phase re-presents black; during the logo phase it drops GPU splash state and re-presents black then logo; an already-installed session and a completed full renderer survive into the resumed loop.

---

## 6. Lifetimes

| Scope | Cleared on |
|-------|-----------|
| Session-lifetime core (primitive registry, `ScriptCtx`, `ScriptRuntime`, Rust-side registries, input/UI/modal group, `PlayerOptions` + options bridge, frontend, net endpoint, audio) | Process exit only. Owned by `Session`, built once post-first-pixel by `Session::build`, then held for the whole run via `App.session`; never recreated. Side-effect-free player-options loading happens pre-window and transfers into that construction. Survives platform suspend. |
| `data_registry.entities` (entity-type descriptors from `ModManifest.entities`) | Engine-global. Survives level unload; survives platform suspend. |
| `data_registry.maps` (mod map catalog from `ModManifest.maps`) | Engine-global. Survives level unload; survives platform suspend. |
| `data_registry.global_reactions` / `global_crossings` / `global_trigger_events` / `global_trigger_pools` (definitions from `ModManifest.reactions` / `crossings` / `triggerEvents` / `triggerPools`) | Engine-global. Survive level unload; survive platform suspend. |
| Active per-level reactions/crossings/trigger events/trigger pools (`data_registry.reactions` / `crossings` / `trigger_events` / `trigger_pools`), accumulator bindings, level-scope UI trees, and frame-end death-event carryover | Level unload. Active sets recompose on the next level load from current globals plus that level's catalog tags. |
| Level world, collision world, environment volumes, fog bridge, light bridge, level sounds, sprite collections, the level's renderer resource set | Level unload; also cleared/dropped by suspend or exit as applicable. An uncommitted load's set is released when the load fails or is cancelled. |
| Shared model cache, add-only sampler maps, install pipelines | The full renderer: they outlive level loads and unloads, and drop with the renderer at suspend or exit; the resumed renderer starts them empty. Model-cache entries also free at a commit that leaves nothing referencing them (`resource_management.md` §8.2). |
| Engine load pool | Boot lifetime: built at the first load dispatch, survives suspend, shut down at exit without waiting. |
| Renderer bloom profile (`ModManifest.render.bloom`) | Engine-global. Survives level load/unload and full-renderer rebuild; re-applied from the manifest by `run_deferred_mod_init` on resume, after the renderer is recreated. A committed staged reload replaces it (no start script restores the default); failed, rejected, or stale reloads leave it alone. |
| Renderer device/queue, window | Dropped on exit; cleared on suspend and rebuilt on resume. (Audio is session-owned — see the session-lifetime row; it survives suspend.) |

Hot reload (debug only) stages entity descriptors, store declarations, the map catalog, and mod-global reaction/crossing/trigger-pool definitions off-thread, then reconciles them on the main thread. Compatible store schemas preserve live values; incompatible changes reject the staged result and preserve the previous descriptors/catalog/globals. Mesh blocks and weapon presentation model paths remain at their installed values until the next level load; ordinary weapon tuning still refreshes live. Complete deletion of a presentation-bearing descriptor is deferred on the same installed-snapshot boundary, even when no live holder is currently observable: network materialization and runtime spawners may still address it during the level. Deletion of a descriptor with no mesh or weapon presentation model takes effect immediately. A successful staged commit atomically replaces the remaining committed snapshots, then recomposes active sets for current level tags without re-rolling the live level. Stale or failed staged results leave committed globals and active sets unchanged. Removed store declarations never clear committed stores.

---

## 7. Manual Lifecycle Checklist

- Launch with no map argument: engine reaches Frontend, paints a world-less frame, no panic.
- Launch with `--mod <campaign>` and no map: frontend menu appears. Confirm declared camera pose, optional background level, and suppressed player controls.
- From that menu: start a catalog map, play, quit to menu, then start again. Confirm no stale backdrop/world residue.
- Bind death to `restartLevel()` in test content: death reloads the active map.
- Launch with no mod frontend: engine fallback frontend menu appears over the world-less Frontend state.
- Launch with a boot map: Splash → Loading → Running, first gameplay frame appears.
- Fresh checkout setup for the `dev-tools` lifecycle trigger: build the generated second dev map first:
  `cargo run -p postretro-level-compiler -- content/dev/maps/combat-demo.map -o content/dev/maps/combat-demo.prl`
- In a `dev-tools` build, trigger `Alt+Shift+L`: unload current level, load `content/dev/maps/combat-demo.prl`, return to Running.
- Confirm no wgpu validation errors during load → unload → load-different.
- Confirm clean visuals after the second load: no stale world geometry, fog, lights, sprites, models, or level sounds.
- With a co-op client connected, change the host's level: the client stays connected through the host's load.

---

## 8. Non-Goals

- Per-entity script lifecycle callbacks (see `entity_model.md` §9)
- Two levels drawn or simulated at once. A load beside a running level (§4) holds its unpublished set resident next to the published one, so at most two sets are resident, but only the published set draws.
- Networked mod sync; runtime mod hot-swap mid-level
- Sandboxing mods from each other (mods share VM contexts and `data_registry` by design)

---

## 9. Planned (not implemented)

None of the following exists in code. Do not treat any of it as current behavior; it is recorded only to anchor future work.

- **Mod discovery / multiple mod roots.** A scan of `content/*/` mod roots plus per-mod load-order resolution. Today one active mod root is selected explicitly with singular `--mod <name>`, by the CLI map path, or by the default dev root.
  - **Consumer: co-op mod adoption.** On a mod-id refusal (`ClosingCause::ModId`) with no explicit mod choice (neither `--mod` nor a map path), the client resolves the host's declared id through discovery to a `content/<folder>` and relaunches once with `--mod <folder>`, keeping its other launch flags. A relaunched client never relaunches again. Discovery must read a mod's declared id without committing mod init: today the id exists only after the start script's `defineMod`. Open: the adopted client's `--app-name` and player-data identity.
- **Mod browser and persisted mod selection.** Engine-native mod browsing, multi-mod selection (`--mods`), and remembering the last selected mod. Singular `--mod` is already the shipped single-mod boot handle.
- **Mod-supplied splash override.** No override hook is active until a splash-override feature ships. The renderer-owned boot splash (§1) consumes only the built-in base asset today; the mod-override consume path is wired but unreachable until the mod system sets it.
- **Seamless level swap** (Epic 25 U3, not built). A production verb for the beside-a-running-level commit order (§4), which today only tests drive, and its swap UX. It owns reconciling a bundle's tiers with a Running-frame change at commit, a pre-commit settle that moves the first frame's streamed reads before the commit (whether the loading-screen order adopts it is open), and per-load level streaming state, so streaming sessions are still created at commit. It also measures whether the load pool, the upload thread and the SH decode pool leave a running level its frame budget.
- **Classname-driven world mesh spawn.** A classname handler replaces the hardwired single-mesh seam (§3 stage 5).
- **Domain folder convention.** Fixed `start-script` entry, `actors/`, `weapons/`, `levels/<name>/<name>.{ts,luau}` auto-discovery, and moving the data script out of the PRL. Today the data script is bundled in the PRL.
