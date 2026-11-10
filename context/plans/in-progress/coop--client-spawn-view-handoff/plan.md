# coop--client-spawn-view-handoff — plan of record

mode: compact
status: active
read at: 3f851eff1 (no `crates/` change since the brief's b215170, so the brief's grounded reads stand)

## Corrections
- Wire table types the hint with `f32` fields → `ServerControlMessage` derives `Eq`, which `f32` cannot satisfy; planning around it by dropping `Eq` (keeping `PartialEq`) on `ServerControlMessage` if nothing needs it, otherwise wrapping the pose in a `PartialEq`-only struct. Layout and fields are unchanged.
- App protocol id moves `PRLC` → `PRLD` (the brief says "moves the app protocol id" without naming it).

## Delegated answers
- Which descriptor class and key bag a pose that no placement backs spawns with — the test substitution names the class beside its pose (an empty key bag); production never yields an unbacked pose until E25 U2 pins its own.

## AC-to-proof

Row ids: R = resolver and reservation, C = client stand-in, M = frontend menu, A = arming, W = wire, X = manual.

| AC | Proof | Status |
|---|---|---|
| R1 install: Relevel then hint before promotion; pawn eye within 1 cm, rotation carries yaw and pitch (O8) | host harness test, install send order + promotion pose | achievable as stated |
| R2 unbacked test pose: hint, stand-in and pawn use it; no reservation; later seat takes lowest free | host harness test with test resolver source | achievable as stated |
| R3 respawn never takes another seat's reservation while one is free; seat with none takes first free | respawn placement test after resolution republish | achievable as stated |
| R4 single-use source consumed → re-promotion and rejoin re-resolve to a player start, reserve, hint before marker (O6) | host harness test, single-use source | achievable as stated |
| R5 spawn stays at hinted pose when another seat takes/frees a placement between hint and promotion | host harness test | achievable as stated |
| R6 live pawn on reserved start at promotion → free placement clear of live pawns + fresh hint | host harness test | achievable as stated |
| R7 two seats resolved before spawn get distinct placements, not host's or held seat's; overflow round-robin warns once each | seat/resolver test with log capture | achievable as stated |
| R8 demoted and held seats keep reservation; joiner skips it; returning seat spawns at hinted pose; demoted-without-reload gets no fresh hint (O10) | host harness test | achievable as stated |
| R9 admission while installed (Running and Settling): Relevel then hint; uncatalogued: hint alone | host harness tests, both poll paths | achievable as stated |
| R10 admission while host loading: no hint; at install Relevel then hint against new level (O3) | host harness test | achievable as stated |
| R11 client revealing after admission: hint precedes participation marker on Control (catalog and uncatalogued) | transport-order test over Control | achievable as stated |
| R12 pre-connect uncatalogued reveal promoted in admission batch: hint after marker, applied unarmed (O11) | host+client harness test | achievable as stated |
| R13 level with no `player_spawn`: no hint, join as today | host harness test | achievable as stated |
| R14 host pawn keeps install's first-player spawn; no joiner assigned it | host harness test | achievable as stated |
| C1 matching hint: install, settle, reveal at hinted eye/yaw/pitch; nonzero pitch fixture | client install/presented-pose test | achievable as stated |
| C2 local `player_spawn` order differs from host's → still host pose | client test | achievable as stated |
| C3 hint in same Control drain as Relevel applies to the new level after unload | client drain test | achievable as stated |
| C4 holding notice after hint (revealed hold included) leaves hint; settles at it (O4) | client test | achievable as stated |
| C5 hint naming other identity or earlier life ignored; stand-in at first spawn | client test | achievable as stated |
| C6 host same-map restart: reload, settle at hint naming new life | client test | achievable as stated |
| C7 hint during Settling retargets; Settling timer not reset | settling test | achievable as stated |
| C8 hint first arriving on reveal frame: that frame at settled pose; moves later while unarmed (O7) | client frame test | achievable as stated |
| C9 Running unarmed fresh hint moves camera before arming | client test | achievable as stated |
| C10 hint after arming moves neither position nor yaw/pitch, incl. overlap re-resolution (O5) | client test | achievable as stated |
| C11 no hint → first-spawn stand-in, no added hold | client test | achievable as stated |
| C12 non-finite component rejected before apply | client hint-store unit test | achievable as stated |
| M1 frontend menu up at host-level install → reveal at hint, menu gone; `--connect` no map arg and replaced map arg | client boot-destination/install test | achievable as stated |
| M2 submenu over frontend menu → both cleared (O15) | client test | achievable as stated |
| M3 in-flight backdrop of host's catalog level adopted → host's level, menu gone, hint (O2) | client test | achievable as stated |
| M4 installed backdrop (Settling or Running) reloads on relevel, settles at hint (O1, O12, O13) | client test | achievable as stated |
| M5 host-named load with no matching hint → first-spawn stand-in, not menu pose (O14) | client test | achievable as stated |
| M6 failed host load or unknown catalog id → frontend menu, no backdrop load; first and later loads (O9, O16, O18) | client test | achievable as stated |
| M7 self-initiated backdrop keeps the menu even if it is the host's last-named level (O17) | client test | achievable as stated |
| M8 host at frontend: no Relevel, no hint, no promotion; on host level start Relevel then hint (O19) | host harness test | achievable as stated |
| M9 un-named backdrop and single-player menu install present the menu pose | presented-pose test | achievable as stated |
| A1 with hint applied, arming frame camera within 1 cm, yaw/pitch unchanged; single-spawn and non-first placement | client arm test | achievable as stated |
| A2 regression: unarmed view turn keeps yaw/pitch at arm | client arm test | achievable as stated |
| W1 round trip; previous app protocol refused at gate 1; no shipped Control encoding changes; net has no internal dep | `crates/net` wire + handshake tests; `layering_invariants_hold` | achievable as stated |
| X1 loopback co-op on `coop-presentation-test` with distinct client yaw, with and without client map arg | owner, in-engine | manual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Wire: append `SpawnPoseHint`, protocol `PRLD`, append guard, round trip, gate-1 refusal (W1) | integrating executor | — | |
| 2 | Host resolver module + reservation + promotion at recorded pose (placement-backed and unbacked spawn entry) + install send; first slice proves R1, R2 | worker (host) | 1 | |
| 3 | Host remaining send sites and re-resolution: admission (world-less and Running), loading-host admission, overlap, single-use, demote/hold, host frontend names no level (R3–R14, M8) | worker (host) | 2 | |
| 4 | Client hint store + stand-in: install, Settling retarget, Running unarmed, arming guards (C1–C12, A1, A2) | worker (client) | 1 | |
| 5 | Client frontend menu: host-named load marker, clear layers, failure fallback without backdrop, installed-backdrop reload (M1–M7, M9) | worker (client) | 4 | |
| 6 | Docs: drop "decided, not yet built" markers, Version gates entry, ledger fix, `docs/` join view note; manual fixture yaw | integrating executor | 2–5 | |
