# release-indirect-validation — plan of record

mode: compact
status: active
read at: c443c91ef

## Corrections
- Upload batching is merged in `0f6f52810`; its changes to cited renderer symbols only replace queue plumbing. Re-read the instance constructors, cull owners, install path, loader check, shader writers, and their consumers; the Decisions remain valid.
- `install_level_payload` changes network parity and navigation before texture installation. Validate the world's unchanged BVH/index slices at entry, before those changes; validate capture before renderer construction or setters. UV normalization changes neither slice.

## Delegated answers
- Install error — renderer-owned typed range error containing the leaf, offset, count and index-buffer length; callers convert it to a failed load. Keep the public geometry installer signature and its debug assertion.
- Manual proof blocks landing: the brief does not permit landing first. Preserve outstanding results in `test-ready` and wait for external proof.

## AC-to-proof

Rows follow the brief's order within Automated and Manual.

| AC | Proof | Status |
|---|---|---|
| A1 release default, both feature sets | policy matrix tests in release + source gate scan | achievable as stated |
| A2 debug default, both feature sets | policy matrix tests in debug + source gate scan | achievable as stated |
| A3 release override 1 | policy matrix | achievable as stated |
| A4 debug override 0 | policy matrix | achievable as stated |
| A5 empty/false override | policy matrix | achievable as stated |
| A6 preserve other bits/env isolation | policy matrix + env source scan | achievable as stated |
| A7 shared constructors/read once | production instance/source scan | achievable as stated |
| A8 aligned load past-end | loader regression with named range message | achievable as stated |
| A9 load exact boundary | loader fixture | achievable as stated |
| A10 load overflow | loader regression | achievable as stated |
| A11 failed install load routes, no side effects | app rejection regression + failure routing/order scan | achievable as stated |
| A12 install past-end, no GPU mutation | pure check + ignored offscreen rejection/state test | achievable as stated |
| A13 install exact boundary/overflow | pure check | achievable as stated |
| A14 install zero leaves | pure check | achievable as stated |
| A15 empty leaf offset boundary, both checks | loader fixtures + pure check | achievable as stated |
| A16 writer ownership and stores | source scans + deliberately invalid scan fixtures | achievable as stated |
| A17 built-ins/composed modules/dispatch | pipeline/source scans | achievable as stated |
| A18 indirect buffer lifetime | install/constructor source scan | achievable as stated |
| A19 whole world index buffer | world draw/upload source scan | achievable as stated |
| M1 submit savings on both maps | paired same-binary release runs, 5+ windows, idle VRAM and cache state | manual-performance |
| M2 sample frames absent/present | symbol-preserving release/debug sample profiles on both maps | manual-performance |
| M3 startup logs including dev-tools | windowed startup logs for build/override matrix | manual-runtime |
| M4 visual parity | owner, spawn and short walk on both maps | manual-visual |

## Tasks

| # | Task | Owner | Depends on | Status |
|---|---|---|---|---|
| 1 | Thin instance-policy slice; build release and attempt hallway paired timing before adding tests | integrating executor | — | pending |
| 2 | Load regressions, pure install check, early failed-load routing and offscreen rejection proof | integrating executor | 1 | pending |
| 3 | Policy tests, invariant comments, source drift guards and invalid scan fixtures | integrating executor | 1, 2 | pending |
| 4 | Focused readiness gate, review/fix loop, final preflight; record all results and external runbook | integrating executor | 2, 3 | pending |

## Runtime constraints
- Instance policy reads once at creation and adds no frame work. Range checks run O(leaves) only at install; existing cull and draw hot paths remain unchanged.
- Cargo runs are owned by the integrating executor and sequential. Check free disk space after each task/numbered step; only caches may be cleared below 15 GB.
