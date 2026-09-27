# coop-trigger-screen-effects

Brief · compact · Epic 23 (adjacent, Phase 1) · reads: `context/lib/networking.md` §Presentation events vs. replicated state, `context/lib/scripting.md` §12, `context/lib/ui.md` §3 · read at 683e363ba

> **Seed.** Found by the E23 U1 draft session. Nothing below is decided yet. Run `/draft-session` on this problem before the brief is written.

## Problem

A defect, surfaced by E23 U1 research. In co-op, a `flashScreen`, `screenShake` or `vignette` reaction fired from a trigger binding never reaches a client. Its cause: trigger-binding reactions run only inside the host's full `simulate_tick`, while clients run `simulate_client_wieldable_tick`. The resulting `screen.*` slots declare `ReplicationScope::None`, and no presentation message carries the effect. Crossing-driven effects (low health) already run on the owning client and are unaffected. When this is done, a trigger-fired screen effect shows on the affected player's own screen in co-op, and that machine's E23 accommodations apply to it (reduce-motion scaling at the pack step, flash limiter).

## Verified facts

- The reactions push `SystemReactionCommand::{FlashScreen, Vignette, ScreenShake}` (`register_system_reaction_primitives`), drained by `App::dispatch_system_commands` into the per-peer decay systems. No command carries an owner or seat.
- Decay ticks run in game logic on every peer. `screen.flash`, `screen.vignette` and `screen.shake` are `ReplicationScope::None`.
- `App::dispatch_state_crossings` is not role-gated, and crossing reactions run on the client after owner-private slots apply.
- Other host-only reaction sources (movement events such as `slide_ended`) may share the gap. Not traced.

## Questions for the session

- Whose screen does a trigger-fired effect belong to: the trigger's activator, every peer inside the volume, or every peer? This is authored policy, and the reaction surface may need to express it.
- Transport: the unreliable Presentation channel (`networking.md` precedent for transient feedback), or client-side trigger evaluation.
- Does the same fix cover the other host-only reaction sources, and which of them exist?
- The host's own screen: today a client's trigger shakes the host's screen too. Is that intended?
