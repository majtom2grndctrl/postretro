// Ghost-link acceptance fixture (maps/ghost-links.map). Luau twin: ghost-links.luau.
// The first block is the brief's level-script example verbatim; the barrier
// puzzle and the battle swap follow. Explicit names and member-ordered lists keep
// the two runtimes' installed manifests equal.
import { defineReaction, getMapEntities, npcs } from "postretro";
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

  // Barrier puzzle: the button in copy B drops the barrier in copy A. Link
  // echo_a lists the barrier's tag, so the player in copy B watches it drop.
  const barrier = getMapEntities("mover", { tag: "vault_barrier" });
  const open = defineReaction("barrier.open", { sequence: barrier.flatMap((door) => door.start()) });

  // Battle swap: the pad arms both seamless swap teleporters, so each player
  // lands in the other copy, facing the monsters only the partner could see.
  // The monsters wake, and the links stop ghosting the partner's projectiles.
  const swap = getMapEntities("teleporter", { tag: "echo_swap" });
  const battle = defineReaction("battle.swap", {
    sequence: [...swap.flatMap((t) => t.arm()), npcs({ tag: "echo_monster" }).update({ aggro: true })],
  });
  // The first crossing disarms both directions, so walking out and back in
  // does not swap again.
  const settle = defineReaction("battle.settle", { sequence: swap.flatMap((t) => t.disarm()) });

  return {
    reactions: [reveal, swell, fight, open, battle, settle],
    triggerEvents: [
      ...getMapEntities("trigger", { tag: "echo_plate" }).map((t) => t.on("enter", [reveal, swell])),
      ...getMapEntities("trigger", { tag: "vault_button" }).map((t) => t.on("enter", [open])),
      ...getMapEntities("trigger", { tag: "swap_pad" }).map((t) => t.on("enter", [battle, fight])),
    ],
    teleportEvents: swap.map((t) => t.on("teleport", [settle])),
  };
}
