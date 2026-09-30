# Slide camera feel

Make an active slide readable through a sustained camera dip and FOV increase, authored in the player descriptor.

## Decisions and invariants

- Add optional `movement.viewFeel.slide`: `eyeDrop` (finite metres >= 0), `fovIncrease` (finite horizontal degrees in [0, 90]), `enterRate` and `exitRate` (finite exponential response rates in [0.1, 240] per second). All four fields required when present. Omission preserves existing presentation.
- Foundation type: `SlideViewParams { eye_drop, fov_increase, enter_rate, exit_rate }`; `ViewFeelParams.slide: Option<SlideViewParams>`, serde default and skip when absent.
- Presentation follows the pawn's current native Sliding state, holds for its duration, then eases to zero on every exit, including crouch, stand, jump, dash, and reconciliation. No movement/collision/aim or wire-state changes.
- A render-rate blend approaches 1 on slide and 0 otherwise with `1-exp(-rate*dt)`. Zero dt holds state. Pawn/descriptor changes and session reset clear state through existing view-feel lifecycle.
- Scale both channels by resolved view-feel scale, including reduced motion. Integrate while scale is zero.
- Compose with existing bob/impulses. Clamp the added downward offset against presented eye-to-feet clearance, retaining at least 0.05 m. Audio, visibility, render and viewmodel consume the same composed eye. Renderer owns final FOV clamp.
- Dev player: eyeDrop 0.15 m, fovIncrease 5 degrees, enterRate 18/sec, exitRate 12/sec. Remove slide impulse FOV kicks so they do not overwhelm the sustained example; retain its pitch/roll kicks.
- Hot path: constant work, no allocation, no simulation writes or new GPU resources.

## Ownership and acceptance

Descriptor track owns foundation descriptor type, JS/Luau parsers, SDK declarations, parser tests and author docs. Runtime track owns camera assembly/evaluator and tests, dev player, durable context, and minimal compile-required fixture updates outside descriptor files.

- JS and Luau agree on the four fields; missing/wrong/non-finite/out-of-range fields fail with descriptor errors. Existing descriptors keep slide view disabled.
- Runtime tests prove held dip/FOV, all non-slide exits, zero dt, frame-rate independence, scale zero, frame-eye composition and feet clearance.
- `cargo check -p postretro`, focused scripting/runtime tests, formatting, clippy and final workspace tests pass. Manual game-feel proof is recorded if no interactive play is available.

No open questions. Single isolated worktree; no extra engine builds for descriptor/review tracks.

## Verification

- Engine compile check passed.
- Descriptor view-feel tests: 43 passed, including JS/Luau precision-boundary rejection.
- Sustained slide/render-eye tests: 7 passed. Followed-pawn descriptor refresh regression: 1 passed.
- Single review fixed strict pre-narrowing validation and interpolated eye-to-feet clearance; no unresolved decisions.
- Formatting and workspace clippy with warnings denied passed.
- Full workspace test gate: 8,642 passed, 0 failed, 30 intentionally ignored. SDK registry snapshots and unchanged wire fixture passed.
- Test compilation used reduced debug information and four build/test workers to bound resource use. Incremental caches were reclaimed when free disk fell below 15 GB; source and other worktrees remained intact.
- Interactive slide/crouch/stand play and subjective feel remain manual proof. Automated camera tests cover held dip/FOV, recovery, preferences, resets and floor clearance.
