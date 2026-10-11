# Playtest parity direction — proposed owner amendment

status: proposed; no Decision or Acceptance change approved by this document
source: `5fbd32328`; [Astra audit](parity-audit.md), [weapon investigation](alternate-fire-audit.md)

## Recommendation

Repair the confirmed weapon execution defect now under the owner's root-fix request. Keep the current brief open. Amend its shared combat-feedback contract to include enemy health/shield bars and participation isolation. Draft separate work for missing world audio and actor pose/timing. Reuse existing briefs for held-flight cleanup, bound presentation removal, reaction composition and persistent firing modes.

## Current brief amendment

Replace Decision 13 / AC 23's health-bar carve-out with these behaviors:

- Any qualifying damage from the host, a client, an NPC, or an absent source enrolls the damaged enemy in one shared host-selected roster. Every participating peer, including the host, consumes that roster. Local cameras and accessibility preferences still control rendering.
- Keep the author's existing maximum-visible and linger values. Select the bounded roster once using host damage time/generation and stable NetworkId ordering. Equal-time overflow selects the same IDs on all peers. Linger starts from original host damage time, independent of receive time and skipped local rendering.
- Publish health/shield/alive facts as bounded expiring state with revision/terminal ordering and recovery after loss. A fact arriving before its entity baseline must recover after the baseline. No client gains Health, AI, damage authority, or a simulated enemy.
- A joining/re-promoted client sees the currently live roster with its remaining lifetime. It never restarts expired bars or replays past damage numbers/sounds. Death/despawn, removal from the roster, and expiry retire bars without stale resurrection.
- Keep XP/rewards, owner health/resources, hit confirmation, screen-effect audiences and private HUD under their existing contracts.
- Isolate disposable Presentation messages by the existing participation generation. A packet sent before demotion must be rejected even if it arrives after re-promotion. Retain unreliable, fire-and-forget damage numbers and commands; do not add replay or reliability to them.
- Permit the necessary coordinated networking layout/version updates and internal API changes, with all producers, consumers, validation and version gates updated together. No new author settings are needed for this baseline.

## Required proof

1. Host, two clients, NPC and absent-source hits produce the same eligible roster and fractions. K+1 equal-time hits select identical IDs. A later hit refreshes the same host expiry even when its final health fraction is unchanged.
2. Loss, duplication, reorder, delayed entity baseline, death before baseline, rendering stalls and level changes cannot permanently strand a stale bar or revive a retired target. Current live state converges through the chosen bounded repair path.
3. Late join and re-promotion apply only the still-live roster and remaining lifetime. Old numbers/commands delivered after a participation change are dropped; fresh messages still work.
4. Select and read facts once per roster update, not once per recipient. State is bounded by authored K plus normal bounded delivery bookkeeping. Measure encoded bytes and update/repair cadence with representative player/target counts; cap pending state and retained revisions across their full lifetime.
5. Run the relevant focused tests, affected independent review and final checks. Rebuild playtest binaries. Confirm shared bars and alternate fire on matching Mac/Windows revisions with host/client roles swapped. Existing M1–M4 still block landing.

## Separate follow-ups

- **World audio:** one bounded frozen cue path for actual jump/land, non-shot weapon lifecycle, AI activity/contact attack and mover sound producers. Preserve owner prediction exclusion, captured sound/anchor facts, local spatialization and no late-join replay. Decide cue delivery/rate/overflow budgets explicitly; keep cosmetic traffic from starving combat/control.
- **Actor presentation:** minimal NPC aim scalars and animation entry identity/time. Preserve locally derived locomotion; synchronize meaningful one-shot age. Carry original projectile launch age where needed. Prove the full producer-to-baseline-to-render-input path. Optional pawn/held-item lights and emitters need a distinct author attachment contract.

These follow-ups are recommendations, not new promoted briefs or authorization to implement their entire scope.

## Weapon repair boundary

Unchanged effective weapon tuning must do no descriptor refresh, activation compilation, Weapon registry write or cancellation. A real tuning/program change still validates atomically and cancels future work under the existing lifecycle. Grants, removals, canonical replacement, unrelated slots, malformed Control, demotion and retained inventory components preserve their existing semantics.

Production snapshot receive must interleave real rifle burst/plasma charging in the regression. Existing tests advanced the activation machinery without executing the destructive receive/materialization seam. The disappearing charge cursor is source-proven; the reported ordinary plasma bolt and firing-owner triple audio still need identity/count evidence. Do not claim the entire observed sequence fixed until that evidence passes.
