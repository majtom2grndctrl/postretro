// Ghost-link acceptance fixture (maps/ghost-links.map). Luau twin: ghost-links.luau.
// A two-player co-op set-piece. The first block is the brief's level-script
// example verbatim; the barrier puzzle, the timed north closet, the split, the
// flip, the return and the final wave follow. The first press of a purple pad
// sends whoever stands on it alone into the other copy; every later press flips
// both players between the copies until the return reunites them. Explicit
// names and member-ordered lists keep the two runtimes' installed manifests
// equal.
import { defineReaction, fire, getMapEntities, npcs, wait } from "postretro";
import { setGhostAudio } from "postretro/audio";

export function setupLevel() {
  const echoes = getMapEntities("ghostLink", { tag: "vault_echo" }); // map order
  const reveal = defineReaction("echo.reveal", {
    sequence: [
      ...echoes.flatMap((l) => l.arm()), // also: l.disarm()
      // Each named field ramps on its own; omitted fields keep their value and ramp. periodMs defaults to 0.
      ...echoes.flatMap((l) => l.setColorFilter({ tint: [1.0, 0.4, 0.3], opacity: 0.6, periodMs: 1500 })),
    ],
  });
  // Narrow what a link shows for one scenario; omitted categories keep their value.
  const fight = defineReaction("echo.fight", { sequence: echoes.flatMap((l) => l.setMembers({ projectiles: false })) });
  // A mod-wide primitive with no target is a whole reaction body, like setSentiment.
  const swell = defineReaction("echo.swell", setGhostAudio({ volume: 0.9, lowPassHz: 5000, periodMs: 1500 }));

  // Barrier puzzle: the button in back room B drops the barrier in copy A.
  // Link echo_a lists the barrier's tag, so the player in copy B watches it drop.
  const barrier = getMapEntities("mover", { tag: "vault_barrier" });
  const open = defineReaction("barrier.open", { sequence: barrier.flatMap((door) => door.start()) });

  // North closet, copy A: its door sinks and its knights wake. The monsters'
  // sealed aggro gate (enabled_on_spawn 0) ignores everything else, damage
  // included. Fifteen seconds after level load, or the first player to walk up
  // to the barrier, whichever comes first. A start on a mover already moving
  // or open does nothing and waking a woken monster changes nothing, so the
  // second path opens nothing again.
  const northDoor = getMapEntities("mover", { tag: "echo_closet_a1" });
  const northCloset = defineReaction("closet.a1", {
    sequence: [...northDoor.flatMap((door) => door.start()), npcs({ tag: "echo_monster_a1" }).update({ aggro: true })],
  });
  // The timer counts from level load, so a host still waiting for a partner
  // may meet the knights alone. The fire step keeps the group wake out of the
  // levelLoad body itself.
  const northTimer = defineReaction("levelLoad", { sequence: [...wait(15000), ...fire(northCloset)] });

  // Split: the first pad press arms both split teleporters, one over each pad,
  // so only whoever stands on a pad crosses, and turns the split pads off. The
  // pad press stands in for a game condition (a timer, a health threshold) and
  // splits once: press it alone from the pad's centre.
  const splitPads = getMapEntities("trigger", { tag: "split_pad" });
  const swapPads = getMapEntities("trigger", { tag: "swap_pad" });
  const split = getMapEntities("teleporter", { tag: "echo_split" });
  const splitArm = defineReaction("split.arm", {
    sequence: [...split.flatMap((t) => t.arm()), ...splitPads.flatMap((p) => p.disarm())],
  });
  // The crossing turns the split off and only then turns the swap pads on. A
  // swap pad armed on the press itself could fire on that same press.
  const splitSettle = defineReaction("split.settle", {
    sequence: [...split.flatMap((t) => t.disarm()), ...swapPads.flatMap((p) => p.arm())],
  });

  // The crossing starts both fights. Copy A's south closet opens at once, so
  // its knight waits for whoever is left in or flips back to copy A.
  const southDoor = getMapEntities("mover", { tag: "echo_closet_a2" });
  const southCloset = defineReaction("closet.a2", {
    sequence: [...southDoor.flatMap((door) => door.start()), npcs({ tag: "echo_monster_a2" }).update({ aggro: true })],
  });
  // Copy B's closets open three seconds after the crossing lands, so the
  // arrival gets a beat to look around before the limitators wake.
  const closetsB = getMapEntities("mover", { tag: "echo_closet_b" });
  const limitators = defineReaction("closet.b", {
    sequence: [
      ...wait(3000),
      ...closetsB.flatMap((door) => door.start()),
      npcs({ tag: "echo_monster_b" }).update({ aggro: true }),
    ],
  });

  // Flip: every swap pad press arms both seamless swap teleporters, so each
  // player in a room lands in the other copy; the links stop ghosting the
  // partner's projectiles. The first crossing disarms both directions, and
  // the next press arms them again. The return turns the swap pads off.
  const swap = getMapEntities("teleporter", { tag: "echo_swap" });
  const flip = defineReaction("swap.arm", { sequence: swap.flatMap((t) => t.arm()) });
  const flipSettle = defineReaction("swap.settle", { sequence: swap.flatMap((t) => t.disarm()) });

  // Final wave: the barrier drops if it still stands, and after a three-second
  // beat the closet in back room A opens and its monsters wake.
  const finalDoors = getMapEntities("mover", { tag: "final_closet_door" });
  const wave = defineReaction("wave.start", {
    sequence: [
      ...barrier.flatMap((door) => door.start()),
      ...wait(3000),
      ...finalDoors.flatMap((door) => door.start()),
      npcs({ tag: "final_wave" }).update({ aggro: true }),
    ],
  });

  // Return: copy B cleared raises a pedestal in back room B and arms its pad. A
  // press arms the classic teleporter over the pad, so only whoever stands
  // there goes to room A. The arrival reunites the players: it disarms the
  // return, turns the flip off for good and starts the wave.
  const returnPedestal = getMapEntities("mover", { tag: "return_pedestal" });
  const returnPads = getMapEntities("trigger", { tag: "return_pad" });
  const returnGate = getMapEntities("teleporter", { tag: "echo_return" });
  const rise = defineReaction("return.rise", {
    sequence: [...returnPedestal.flatMap((m) => m.start()), ...returnPads.flatMap((p) => p.arm())],
  });
  const clearB = defineReaction("clear.b", { progress: { tag: "echo_monster_b", at: 1.0, fire: "return.rise" } });
  const returnArm = defineReaction("return.arm", { sequence: returnGate.flatMap((t) => t.arm()) });
  const returnSettle = defineReaction("return.settle", {
    sequence: [
      ...returnGate.flatMap((t) => t.disarm()),
      ...swapPads.flatMap((p) => p.disarm()),
      ...swap.flatMap((t) => t.disarm()),
    ],
  });

  return {
    reactions: [
      reveal, swell, fight, open, northCloset, northTimer, splitArm, splitSettle,
      southCloset, limitators, flip, flipSettle, wave, rise, clearB, returnArm, returnSettle,
    ],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal, swell])),
      ...getMapEntities("trigger", { tag: "vault_button" }).map((t) => t.on("enter", [open])),
      ...getMapEntities("trigger", { tag: "barrier_approach" }).map((t) => t.on("enter", [northCloset])),
      ...splitPads.map((t) => t.on("enter", [splitArm])),
      ...swapPads.map((t) => t.on("enter", [flip, fight])),
      ...returnPads.map((t) => t.on("enter", [returnArm])),
    ],
    teleportEvents: [
      ...split.map((t) => t.on("teleport", [splitSettle, southCloset, limitators])),
      ...swap.map((t) => t.on("teleport", [flipSettle])),
      ...returnGate.map((t) => t.on("teleport", [returnSettle, wave])),
    ],
  };
}
