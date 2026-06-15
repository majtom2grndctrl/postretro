import {
  type NamedReactionDescriptor,
  appendText,
  backspaceText,
  closeDialog,
  defineReaction,
  flashScreen,
  onStateCrossing,
  playSound,
  screenShake,
  showDialog,
  vignette,
  world,
} from "postretro";
import { tabsDemo } from "./tabs-demo";

export function setupLevel(_ctx: unknown) {
  const reactions: NamedReactionDescriptor[] = [];

  // Arena 1: angular sweep from the NW corner, counterclockwise.
  const arena1Raw = world.query({ component: "light", tag: "arena_1_light" });
  if (arena1Raw.length > 0) {
    let centroidX = 0,
      centroidZ = 0;
    for (const light of arena1Raw) {
      centroidX += light.position.x;
      centroidZ += light.position.z;
    }
    centroidX /= arena1Raw.length;
    centroidZ /= arena1Raw.length;

    const lightsWithAngle = arena1Raw.map((light) => {
      const dx = light.position.x - centroidX;
      const dz = light.position.z - centroidZ;
      return { light, angle: Math.atan2(dz, dx) };
    });

    // Anchor at the NW corner: the light with the highest z (westernmost).
    const startAngle = lightsWithAngle.reduce((best, cur) =>
      cur.light.position.z > best.light.position.z ? cur : best,
    ).angle;

    const TWO_PI = 2 * Math.PI;
    lightsWithAngle.sort((a, b) => {
      const da = (a.angle - startAngle + TWO_PI) % TWO_PI;
      const db = (b.angle - startAngle + TWO_PI) % TWO_PI;
      return da - db;
    });

    const pulseDurationMs = 300;
    const lightSpacingMs = 150;
    const cyclePauseMs = 2000;
    const N = lightsWithAngle.length;
    const periodMs = (N - 1) * lightSpacingMs + pulseDurationMs + cyclePauseMs;
    const pulseFraction = pulseDurationMs / periodMs;

    const SAMPLES = 32;
    const brightness: number[] = [];
    for (let i = 0; i < SAMPLES; i++) {
      const t = i / SAMPLES;
      brightness.push(
        t < pulseFraction ? Math.sin((t / pulseFraction) * Math.PI) : 0,
      );
    }

    const steps = lightsWithAngle.map(({ light }, i) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs,
        phase: (i * lightSpacingMs) / periodMs,
        playCount: null,
        startActive: true,
        brightness,
        color: null,
        direction: null,
      },
    }));

    reactions.push(defineReaction("levelLoad", { sequence: steps }));
  }

  // Arena 2: west-wall wave, south → north (descending engine-x order).
  const arena2Raw = world.query({ component: "light", tag: "arena_wave_2" });
  if (arena2Raw.length > 0) {
    const sorted = [...arena2Raw].sort((a, b) => b.position.x - a.position.x);

    const pulseDurationMs = 200;
    const lightSpacingMs = 50;
    const cyclePauseMs = 2000;
    const N = sorted.length;
    const periodMs = (N - 1) * lightSpacingMs + pulseDurationMs + cyclePauseMs;
    const pulseFraction = pulseDurationMs / periodMs;

    const SAMPLES = 32;
    const brightness: number[] = [];
    for (let i = 0; i < SAMPLES; i++) {
      const t = i / SAMPLES;
      brightness.push(
        t < pulseFraction ? Math.sin((t / pulseFraction) * Math.PI) : 0,
      );
    }

    const steps = sorted.map((light, i) => ({
      id: light.id,
      primitive: "setLightAnimation" as const,
      args: {
        periodMs,
        phase: (i * lightSpacingMs) / periodMs,
        playCount: null,
        startActive: true,
        brightness,
        color: null,
        direction: null,
      },
    }));

    reactions.push(defineReaction("levelLoad", { sequence: steps }));
  }

  // Fog demo: both fog entity types in the map carry the "pulse_fog" tag,
  // so the tag-targeted scatter primitive and the per-id fog.pulse sequence
  // both demonstrate cross-subtype dispatch (fog_volume + fog_lamp hit together).
  const fogs = world.query({ component: "fog_volume", tag: "pulse_fog" });
  if (fogs.length > 0) {
    // Tag-targeted Primitive: one descriptor, batch-applied to every
    // "pulse_fog" volume regardless of entity subtype.
    reactions.push(
      defineReaction("levelLoad", {
        primitive: "setFogScatter",
        tag: "pulse_fog",
        args: { scatter: 0.4 },
      }),
    );

    // Per-id Sequence: a single `setFogAnimation` step carrying a sine
    // density curve, evaluated per-frame across `periodMs` on each
    // matched volume.
    for (const fog of fogs) {
      const steps = fog.pulse({ min: 0.2, max: 1.0, periodMs: 5000 });
      reactions.push(defineReaction("levelLoad", { sequence: steps }));
    }
  }

  // M13 Goal E demo: HUD reacts to game state. When the authoritative
  // `player.health` slot crosses below 20% of its 100 HP max, fire a red
  // screen flash (engine-decayed `screen.flash` surface) and a one-shot alert
  // sound on the SFX bus. The same 20% threshold drives the HUD health bar's
  // critical (red) styleRanges band — the bar shows the band, the crossing
  // fires the flash + sound. `flashScreen`/`playSound` are system reactions
  // (no entity tag); they enqueue typed commands the app drains each frame.
  //
  // M13 Goal SE demo: the same low-health crossing ALSO fires a red-tinted
  // vignette (edges darken/tint then decay, center untouched) and a screen
  // shake (decaying oscillation returning to exact center). All three compose
  // in the single post-UI resolve pass and pause with game logic. There is no
  // entity-hit event seam, so `onStateCrossing` is the trigger surface.
  reactions.push(
    defineReaction("lowHealthFlash", flashScreen([1.0, 0.0, 0.0, 0.5], 250)),
    defineReaction("lowHealthVignette", vignette(0.7, 400, [0.6, 0.0, 0.0])),
    defineReaction("lowHealthShake", screenShake(12, 300)),
    defineReaction("lowHealthAlert", playSound("sfx/test_tone", "sfx")),
  );

  // M13 Goal F (Task 5) demo: the engine pause menu's RESUME button fires the
  // `resumePauseMenu` reaction on activation (gamepad confirm / click), which
  // `closeDialog` pops off the modal stack — the same pop the engine `nav.menu`
  // toggle and `nav.cancel` perform. The button's `onPress` name must match this
  // reaction name. (The menu tree itself and the volume slider are engine-owned
  // demo descriptors; this reaction is the script half of the Resume button.)
  reactions.push(defineReaction("resumePauseMenu", closeDialog()));

  // M13 Text-Entry (Task 4) demo: the pause menu's "ENTER TEXT" button fires
  // `openTextEntry`, which `showDialog`-pushes the engine-shipped on-screen
  // keyboard (registered under the name "keyboard") carrying `onTextEntryCommit`
  // as its commit reaction. On commit (the on-screen `done` key OR a hardware
  // Enter), the engine fires `onTextEntryCommit` — an observable `playSound` — so
  // commit is distinguishable from cancel (`nav.cancel` pops without firing it).
  // The pause menu's `text` row binds `ui.textEntry` DIRECTLY, so the entered
  // string shows there as it is typed on either input path.
  reactions.push(
    defineReaction(
      "openTextEntry",
      showDialog("keyboard", "onTextEntryCommit"),
    ),
    defineReaction("onTextEntryCommit", playSound("sfx/test_tone", "sfx")),
  );

  // Per-key named reactions the on-screen keyboard's letter/digit/space buttons
  // reference (`onPress`). Each appends its character to the writable engine slot
  // `ui.textEntry`; backspace pops one grapheme. These are DATA — the keyboard
  // JSON references them by name, so editing the layout (adding/removing keys)
  // needs no Rust change, only matching reaction names here. The `done` key does
  // NOT appear here: its `onPress` is the reserved `ui.commitTextEntry` sentinel
  // the engine intercepts to reach the shared commit seam (Task 3).
  const TEXT_ENTRY_SLOT = "ui.textEntry";
  const keyChars = "abcdefghijklmnopqrstuvwxyz0123456789".split("");
  for (const ch of keyChars) {
    reactions.push(defineReaction(`kbAppend_${ch}`, appendText(TEXT_ENTRY_SLOT, ch)));
  }
  reactions.push(defineReaction("kbAppend_space", appendText(TEXT_ENTRY_SLOT, " ")));
  reactions.push(defineReaction("kbBackspace", backspaceText(TEXT_ENTRY_SLOT)));

  const crossings = [
    onStateCrossing("player.health", { below: 20, max: 100 }, [
      "lowHealthFlash",
      "lowHealthVignette",
      "lowHealthShake",
      "lowHealthAlert",
    ]),
  ];

  // M13 G2 demo: the reactive-UI tabs strip (localState cell + role:"tablist" +
  // predicate-bound highlight/selected + Switch content swap). Its named
  // `cellWrite` reactions merge into this level's reaction registry and its
  // `alwaysOn` tree composes as a HUD-layer base every frame.
  const tabs = tabsDemo();
  reactions.push(...tabs.reactions);

  return { reactions, crossings, uiTrees: tabs.uiTrees };
}
