// Ghost-link acceptance fixture (maps/ghost-links.map). Luau twin: ghost-links.luau.
// The first block is the brief's level-script example verbatim; the barrier
// puzzle, the split, the battle swap, the return and the final wave follow. The
// first press of a purple pad sends whoever stands on it alone into the other
// copy; every later press swaps both players. Explicit names and member-ordered
// lists keep the two runtimes' installed manifests equal.
import { defineReaction, getMapEntities, npcs, wait } from "postretro";
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

  // Battle swap: the pad arms both seamless swap teleporters, so each player
  // lands in the other copy, facing the monsters only the partner could see.
  // The closet doors in both copies sink and the monsters wake; the links stop
  // ghosting the partner's projectiles. The monsters' sealed aggro gate
  // (enabled_on_spawn 0) ignores everything else, damage included.
  const swap = getMapEntities("teleporter", { tag: "echo_swap" });
  const closets = getMapEntities("mover", { tag: "echo_closet_door" });
  const battle = defineReaction("battle.swap", {
    sequence: [
      ...swap.flatMap((t) => t.arm()),
      ...closets.flatMap((door) => door.start()),
      npcs({ tag: "echo_monster" }).update({ aggro: true }),
    ],
  });
  // The first crossing disarms both directions, so walking out and back in
  // does not swap again.
  const settle = defineReaction("battle.settle", { sequence: swap.flatMap((t) => t.disarm()) });

  // Final wave: the barrier drops, and after a three-second beat the closet in
  // back room A opens and its monsters wake. It turns the ready button off. A
  // start on a mover already moving or open does nothing, so whichever path
  // fires it second starts nothing again.
  const readyPads = getMapEntities("trigger", { tag: "ready_pad" });
  const finalDoors = getMapEntities("mover", { tag: "final_closet_door" });
  const wave = defineReaction("wave.start", {
    sequence: [
      ...readyPads.flatMap((p) => p.disarm()),
      ...barrier.flatMap((door) => door.start()),
      ...wait(3000),
      ...finalDoors.flatMap((door) => door.start()),
      npcs({ tag: "final_wave" }).update({ aggro: true }),
    ],
  });

  // Ready button: copy A cleared raises a pedestal in the north closet the
  // knights came from and arms its pad. A press starts the wave.
  const readyPedestal = getMapEntities("mover", { tag: "ready_pedestal" });
  const ready = defineReaction("ready.rise", {
    sequence: [...readyPedestal.flatMap((m) => m.start()), ...readyPads.flatMap((p) => p.arm())],
  });
  const clearA = defineReaction("clear.a", { progress: { tag: "echo_monster_a", at: 1.0, fire: "ready.rise" } });

  // Return: copy B cleared raises a pedestal in back room B and arms its pad. A
  // press arms the classic teleporter over the pad, so only whoever stands
  // there goes to room A; the arrival disarms it and starts the wave.
  const returnPedestal = getMapEntities("mover", { tag: "return_pedestal" });
  const returnPads = getMapEntities("trigger", { tag: "return_pad" });
  const returnGate = getMapEntities("teleporter", { tag: "echo_return" });
  const rise = defineReaction("return.rise", {
    sequence: [...returnPedestal.flatMap((m) => m.start()), ...returnPads.flatMap((p) => p.arm())],
  });
  const clearB = defineReaction("clear.b", { progress: { tag: "echo_monster_b", at: 1.0, fire: "return.rise" } });
  const returnArm = defineReaction("return.arm", { sequence: returnGate.flatMap((t) => t.arm()) });
  const returnSettle = defineReaction("return.settle", { sequence: returnGate.flatMap((t) => t.disarm()) });

  return {
    reactions: [
      reveal, swell, fight, open, splitArm, splitSettle, battle, settle,
      wave, ready, clearA, rise, clearB, returnArm, returnSettle,
    ],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal, swell])),
      ...getMapEntities("trigger", { tag: "vault_button" }).map((t) => t.on("enter", [open])),
      ...splitPads.map((t) => t.on("enter", [splitArm])),
      ...swapPads.map((t) => t.on("enter", [battle, fight])),
      ...readyPads.map((t) => t.on("enter", [wave])),
      ...returnPads.map((t) => t.on("enter", [returnArm])),
    ],
    teleportEvents: [
      ...split.map((t) => t.on("teleport", [splitSettle])),
      ...swap.map((t) => t.on("teleport", [settle])),
      ...returnGate.map((t) => t.on("teleport", [returnSettle, wave])),
    ],
  };
}
