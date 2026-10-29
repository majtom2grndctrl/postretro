// Ghost-link acceptance fixture (maps/ghost-links.map). Luau twin: ghost-links.luau.
// A two-player co-op set-piece: both players start in copy A. The reveal, the
// barrier puzzle, the timed north closet, the split, the flip, the return and
// the final wave follow. The first press of copy A's purple pad sends whoever
// stands on it alone into copy B; every later press flips both players between
// the copies until the return reunites them. Explicit names and member-ordered
// lists keep the two runtimes' installed manifests equal.
import { defineReaction, fire, getMapEntities, npcs, wait } from "postretro";
import { setGhostAudio } from "postretro/audio";
import { playSound } from "postretro/ui";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  // One body: link steps, then the bus swell, a system step. An array is a
  // body, and nested arrays flatten.
  const reveal = defineReaction("echo.reveal", [
    echoes.map((l) => l.arm()), // also: l.disarm()
    // Each named field ramps on its own; omitted fields keep their value and ramp. periodMs defaults to 0.
    echoes.map((l) => l.setColorFilter({ tint: [1.0, 0.4, 0.3], opacity: 0.6, periodMs: 1500 })),
    setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }),
  ]);
  // Narrow what a link shows for one scenario; omitted categories keep their value.
  const fight = defineReaction("echo.fight", echoes.map((l) => l.setMembers({ projectiles: false })));

  // Barrier puzzle: the button in back room B drops the barrier in copy A.
  // Link echo_a lists the barrier's tag, so the player in copy B watches it drop.
  // The same press raises the return button (below).
  const barrier = getMapEntities("mover", { tag: "vault_barrier" });
  const open = defineReaction("barrier.open", barrier.map((door) => door.start()));

  // North closet, copy A: its door sinks and its two knights wake. The monsters'
  // sealed aggro gate (enabled_on_spawn 0) ignores everything else, damage
  // included. Fifteen seconds after level load, or the first player to walk up
  // to the barrier, whichever comes first. A start on a mover already moving
  // or open does nothing and waking a woken monster changes nothing, so the
  // second path opens nothing again.
  const northCloset = defineReaction("closet.a1", [
    getMapEntities("mover", { tag: "echo_closet_a1" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_a1" }).update({ aggro: true }),
  ]);
  // The timer counts from level load, so a host still waiting for a partner
  // may meet the knights alone. The host opens the closet (`fire` reuses the
  // reaction the barrier approach fires too); each connected machine sounds
  // the klaxon once, on the host's timeline, and a player who joins later
  // never hears it.
  const northTimer = defineReaction("levelLoad", [
    wait(15000),
    fire(northCloset),
    playSound("sfx/test_tone"),
  ]);

  // Split: the first pad press arms both split teleporters, one over each pad,
  // so only whoever stands on a pad crosses, and turns the split pads off. Both
  // players start in copy A, so the crossing is always copy A's, tagged
  // split_to_b. The pad press stands in for a game condition (a timer, a health
  // threshold) and splits once: press it alone from the pad's centre.
  const splitPads = getMapEntities("trigger", { tag: "split_pad" });
  const swapPads = getMapEntities("trigger", { tag: "swap_pad" });
  const split = getMapEntities("teleporter", { tag: "echo_split" });
  const splitToB = getMapEntities("teleporter", { tag: "split_to_b" });
  const splitArm = defineReaction("split.arm", [split.map((t) => t.arm()), splitPads.map((p) => p.disarm())]);
  // The crossing turns the split off and only then turns the swap pads on. A
  // swap pad armed on the press itself could fire on that same press.
  const splitSettle = defineReaction("split.settle", [split.map((t) => t.disarm()), swapPads.map((p) => p.arm())]);

  // The crossing into copy B starts both fights in one body. Copy A's south
  // closet opens at once, so its two knights wait for whoever is left in or
  // flips back to copy A; copy B's closets open three seconds later, so the
  // arrival gets a beat to look around before the limitators wake.
  const splitLanded = defineReaction("split.landed", [
    getMapEntities("mover", { tag: "echo_closet_a2" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_a2" }).update({ aggro: true }),
    wait(3000),
    getMapEntities("mover", { tag: "echo_closet_b" }).map((door) => door.start()),
    npcs({ tag: "echo_monster_b" }).update({ aggro: true }),
  ]);

  // Flip: every swap pad press arms both seamless swap teleporters, so each
  // player in a room lands in the other copy; the links stop ghosting the
  // partner's projectiles. The first crossing disarms both directions, and
  // the next press arms them again. The return turns the swap pads off.
  const swap = getMapEntities("teleporter", { tag: "echo_swap" });
  const flip = defineReaction("swap.arm", swap.map((t) => t.arm()));
  const flipSettle = defineReaction("swap.settle", swap.map((t) => t.disarm()));

  // Return: the barrier button raises a pedestal out of back room B's floor at
  // once and arms its pad, whether or not copy B is clear. A press arms the
  // classic teleporter over the pad, so only whoever stands there goes to room
  // A. The arrival reunites the players: it disarms the return and turns the
  // flip off for good, then after a three-second beat the closet in back room
  // A opens and the final wave wakes. The barrier is already down: the press
  // that raised the return button dropped it.
  const returnPedestal = getMapEntities("mover", { tag: "return_pedestal" });
  const returnPads = getMapEntities("trigger", { tag: "return_pad" });
  const returnGate = getMapEntities("teleporter", { tag: "echo_return" });
  const rise = defineReaction("return.rise", [returnPedestal.map((m) => m.start()), returnPads.map((p) => p.arm())]);
  const returnArm = defineReaction("return.arm", returnGate.map((t) => t.arm()));
  const returnSettle = defineReaction("return.settle", [
    returnGate.map((t) => t.disarm()),
    swapPads.map((p) => p.disarm()),
    swap.map((t) => t.disarm()),
    wait(3000),
    getMapEntities("mover", { tag: "final_closet_door" }).map((door) => door.start()),
    npcs({ tag: "final_wave" }).update({ aggro: true }),
  ]);

  return {
    reactions: [
      reveal, fight, open, northCloset, northTimer, splitArm, splitSettle,
      splitLanded, flip, flipSettle, rise, returnArm, returnSettle,
    ],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal])),
      ...getMapEntities("trigger", { tag: "vault_button" }).map((t) => t.on("enter", [open, rise])),
      ...getMapEntities("trigger", { tag: "barrier_approach" }).map((t) => t.on("enter", [northCloset])),
      ...splitPads.map((t) => t.on("enter", [splitArm])),
      ...swapPads.map((t) => t.on("enter", [flip, fight])),
      ...returnPads.map((t) => t.on("enter", [returnArm])),
    ],
    teleportEvents: [
      ...split.map((t) => t.on("teleport", [splitSettle])),
      ...splitToB.map((t) => t.on("teleport", [splitLanded])),
      ...swap.map((t) => t.on("teleport", [flipSettle])),
      ...returnGate.map((t) => t.on("teleport", [returnSettle])),
    ],
  };
}
