import {
  defineMapCatalog,
  defineReaction,
  type ModMapEntry,
  type NamedReactionDescriptor,
} from "postretro";
import {
  Button,
  CLOSE_DIALOG_ACTION,
  EXIT_TO_DESKTOP_ACTION,
  Glyph,
  Grid,
  HStack,
  OPEN_CONTROLS_ACTION,
  QUIT_TO_MENU_ACTION,
  Slider,
  Switch,
  Text,
  Tree,
  VStack,
  accessibilityAction,
  bindState,
  displayModeAction,
  defineUiTree,
  getGameState,
  loadLevel,
  openMenu,
  stateEquals,
  ui,
  updateState,
  type Predicate,
  type ValueTextCase,
  type WidgetDescriptor,
} from "postretro/ui";

const TITLE_MENU_NAME = "frontend.menuTree";
const LEVEL_SELECT_MENU_NAME = "frontend.devLevelSelect";
const OPTIONS_MENU_NAME = "frontend.options";

const COLOR_ACCENT: [number, number, number, number] = [0.12, 0.72, 0.4, 1.0];
const COLOR_INACTIVE: [number, number, number, number] = [0.12, 0.16, 0.2, 1.0];
const COLOR_MUTED: [number, number, number, number] = [0.58, 0.68, 0.72, 1.0];
const COLOR_PANEL: [number, number, number, number] = [0.018, 0.026, 0.039, 0.94];

export const mapCatalog = defineMapCatalog([
  {
    id: "campaign-test",
    path: "maps/campaign-test.prl",
    name: "Moody Vibe Test",
    tags: ["campaign", "recommended"],
  },
  {
    id: "kinematic-platform",
    path: "maps/kinematic-platform.prl",
    name: "Moving Platforms Test",
    tags: ["platform", "test"],
  },
  {
    id: "movement-feel",
    path: "maps/movement-feel.prl",
    name: "Combat Arena Test",
    tags: ["combat", "movement", "recommended"],
  },
  {
    id: "stress-warren-hallway-inspection",
    path: "maps/stress-warren-hallway-inspection.prl",
    name: "Stress Test",
    tags: ["stress", "test"],
  },
  {
    id: "combat-demo",
    path: "maps/combat-demo.prl",
    name: "Combat + Emissive Test",
    tags: ["combat", "emissive", "recommended"],
    // Overrides the mod-wide loading pool for this map (it is also the
    // frontend backdrop, so its loading screen shows at every boot).
    loadingTree: "dev.loading.combatDemo",
  },
  {
    id: "splash-damage-demo",
    path: "maps/splash-damage-demo.prl",
    name: "Rocket Splash Damage Test",
    tags: ["combat", "test"],
  },
]);

function hasTag(entry: ModMapEntry, tag: string): boolean {
  return entry.tags?.includes(tag) ?? false;
}

function startReactionName(entry: ModMapEntry): string {
  return `frontend.start.${entry.id}`;
}

export const frontendStartReactions = mapCatalog.map((entry) =>
  defineReaction(startReactionName(entry), loadLevel(entry.id)),
);

function levelButton(entry: ModMapEntry) {
  return Button({
    id: `start-${entry.id}`,
    label: entry.name,
    onPress: startReactionName(entry),
  });
}

// A column of level buttons that scrolls once it outgrows the screen. Scroll
// opens no focus group, so both columns' buttons share the level select's
// spatial group: Left and Right cross between columns.
function section(title: string, entries: ModMapEntry[]) {
  return VStack({ gap: 6, align: "stretch" }, [
    Text({ content: title, fontSize: 16 }),
    VStack({ gap: 6, align: "stretch", scroll: { maxHeight: 360 } }, entries.map(levelButton)),
  ]);
}

/// Device-aware prompts: each glyph follows the player's last device and
/// their bindings.
function promptRow() {
  return HStack({ gap: 8, align: "center" }, [
    Glyph({ command: "nav_confirm" }),
    Text({ content: "SELECT", fontSize: 14, color: COLOR_MUTED }),
    Glyph({ command: "nav_cancel" }),
    Text({ content: "BACK", fontSize: 14, color: COLOR_MUTED }),
  ]);
}

function mapsTagged(tag: string): ModMapEntry[] {
  return mapCatalog.filter((entry) => hasTag(entry, tag));
}

export const devLevelSelectMenu = defineUiTree({
  name: LEVEL_SELECT_MENU_NAME,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: `start-${mapCatalog[0].id}`,
      accessibleName: "Dev level select",
      role: "group",
    },
    VStack(
      {
        gap: 14,
        padding: 18,
        align: "stretch",
        fill: COLOR_PANEL,
        focus: { policy: "linear", wrap: true },
      },
      [
        HStack({ gap: 18, align: "start", focus: { policy: "spatial" } }, [
          section("Recommended", mapsTagged("recommended")),
          section("Development Tests", mapsTagged("test")),
        ]),
        Button({
          id: "levelSelectBack",
          label: "BACK",
          onPress: CLOSE_DIALOG_ACTION,
        }),
        promptRow(),
      ],
    ),
  ),
});

// Dev EXIT and QUIT confirmations: each opens a dialog whose initial focus is
// the safe choice, so a second confirm pressed on the next frame cancels.
const EXIT_CONFIRM_NAME = "dev.exitConfirm";
const QUIT_CONFIRM_NAME = "dev.quitConfirm";

function confirmDialog(name: string, prompt: string, label: string, action: string, idPrefix: string) {
  return defineUiTree({
    name,
    tree: Tree(
      {
        anchor: "center",
        offset: [0, 0],
        captureMode: "capture",
        // The safe choice: a repeated confirm never takes the action.
        initialFocus: `${idPrefix}Cancel`,
        accessibleName: prompt,
        role: "group",
      },
      VStack(
        {
          gap: 14,
          padding: 20,
          align: "start",
          fill: COLOR_PANEL,
          focus: { policy: "linear" },
        },
        [
          Text({ content: prompt, fontSize: 22, color: COLOR_ACCENT }),
          HStack({ gap: 12, padding: 0, align: "start" }, [
            Button({ id: `${idPrefix}Cancel`, label: "CANCEL", onPress: CLOSE_DIALOG_ACTION }),
            Button({ id: `${idPrefix}Confirm`, label, onPress: action }),
          ]),
        ],
      ),
    ),
  });
}

export const exitConfirm = confirmDialog(
  EXIT_CONFIRM_NAME,
  "EXIT TO DESKTOP?",
  "EXIT",
  EXIT_TO_DESKTOP_ACTION,
  "exitConfirm",
);

export const quitConfirm = confirmDialog(
  QUIT_CONFIRM_NAME,
  "QUIT TO THE MAIN MENU?",
  "QUIT",
  QUIT_TO_MENU_ACTION,
  "quitConfirm",
);

/** EXIT buttons press this: it opens the exit confirmation. */
export const askExit = defineReaction("dev.askExit", openMenu(EXIT_CONFIRM_NAME));
/** QUIT TO MENU buttons press this: it opens the quit confirmation. */
export const askQuit = defineReaction("dev.askQuit", openMenu(QUIT_CONFIRM_NAME));

const openPlay = defineReaction("frontend.openPlay", openMenu(LEVEL_SELECT_MENU_NAME));
/** Opens the tabbed options screen; the pause menu opens it too. */
export const openOptions = defineReaction("frontend.openOptions", openMenu(OPTIONS_MENU_NAME));

export const frontendMenu = defineUiTree({
  name: TITLE_MENU_NAME,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: "frontendPlay",
      accessibleName: "PostRetro title menu",
      role: "group",
    },
    VStack(
      {
        gap: 12,
        padding: 24,
        align: "stretch",
        fill: COLOR_PANEL,
        focus: { policy: "linear", wrap: true },
      },
      [
        Text({ content: "POSTRETRO", fontSize: 36, color: COLOR_ACCENT }),
        Button({ id: "frontendPlay", label: "PLAY", onPress: openPlay }),
        Button({ id: "frontendOptions", label: "OPTIONS", onPress: openOptions }),
        Button({ id: "frontendExit", label: "EXIT", onPress: askExit }),
      ],
    ),
  ),
});

const { options, window } = getGameState();

const WINDOW_MODE_CHOICES = [
  { value: "windowed", id: "optionsWindowed", label: "WINDOWED" },
  { value: "borderless", id: "optionsBorderless", label: "BORDERLESS" },
  { value: "exclusive", id: "optionsExclusive", label: "EXCLUSIVE" },
] as const;

/// Render resolution choices, in display order. ASCII fractions render in every
/// bundled typeface.
const RENDER_RESOLUTION_CHOICES = [
  { value: "auto", id: "optionsRenderResolutionAuto", label: "AUTO" },
  { value: "native", id: "optionsRenderResolutionNative", label: "NATIVE" },
  { value: "half", id: "optionsRenderResolutionHalf", label: "1/2" },
  { value: "third", id: "optionsRenderResolutionThird", label: "1/3" },
  { value: "quarter", id: "optionsRenderResolutionQuarter", label: "1/4" },
] as const;

const optionReactions: NamedReactionDescriptor[] = [
  ...WINDOW_MODE_CHOICES.map(({ value }) =>
    defineReaction(`frontend.options.windowMode.${value}`, updateState(options.windowMode, value)),
  ),
  defineReaction("frontend.options.invertY.off", updateState(options.invertY, false)),
  defineReaction("frontend.options.invertY.on", updateState(options.invertY, true)),
  defineReaction("frontend.options.crouchMode.hold", updateState(options.crouchMode, "hold")),
  defineReaction("frontend.options.crouchMode.toggle", updateState(options.crouchMode, "toggle")),
  defineReaction("frontend.options.sprintMode.hold", updateState(options.sprintMode, "hold")),
  defineReaction("frontend.options.sprintMode.toggle", updateState(options.sprintMode, "toggle")),
  defineReaction("frontend.options.gamepadInvertY.off", updateState(options.gamepadInvertY, false)),
  defineReaction("frontend.options.gamepadInvertY.on", updateState(options.gamepadInvertY, true)),
  defineReaction(
    "frontend.options.swapConfirmCancel.off",
    updateState(options.swapConfirmCancel, false),
  ),
  defineReaction(
    "frontend.options.swapConfirmCancel.on",
    updateState(options.swapConfirmCancel, true),
  ),
  defineReaction("frontend.options.shadowQuality.low", updateState(options.shadowQuality, "low")),
  defineReaction(
    "frontend.options.shadowQuality.medium",
    updateState(options.shadowQuality, "medium"),
  ),
  defineReaction("frontend.options.shadowQuality.high", updateState(options.shadowQuality, "high")),
  defineReaction("frontend.options.fogQuality.low", updateState(options.fogQuality, "low")),
  defineReaction(
    "frontend.options.fogQuality.medium",
    updateState(options.fogQuality, "medium"),
  ),
  defineReaction("frontend.options.fogQuality.high", updateState(options.fogQuality, "high")),
  defineReaction(
    "frontend.options.surfaceDepthQuality.off",
    updateState(options.surfaceDepthQuality, "off"),
  ),
  defineReaction(
    "frontend.options.surfaceDepthQuality.on",
    updateState(options.surfaceDepthQuality, "on"),
  ),
  ...RENDER_RESOLUTION_CHOICES.map(({ value }) =>
    defineReaction(
      `frontend.options.renderResolution.${value}`,
      updateState(options.renderResolution, value),
    ),
  ),
];

const accessibility = getGameState().accessibility;

/// A `[0, 1]` accessibility slider bound to its working copy.
function unitSlider(id: string, labelledBy: string, bind: typeof options.screenShakeScale, step: number) {
  return optionValue(
    Slider({
      id,
      labelledBy,
      bind,
      min: 0,
      max: 1,
      step,
      valueDisplay: { min: 0, max: 100, suffix: "%", decimalPlaces: 0 },
      capturesNav: ["nav.left", "nav.right"],
    }),
  );
}

function radioChoice(id: string, label: string, checked: Predicate, onPress: string) {
  return Button({
    id,
    label,
    role: "radio",
    checked,
    bind: checked,
    styleRanges: {
      max: 1,
      entries: [{ upTo: 0, color: COLOR_INACTIVE }, { color: COLOR_ACCENT }],
    },
    onPress,
  });
}

/// An OFF/ON radio pair for a boolean option, firing
/// `frontend.options.<field>.off|on`.
function offOnChoices(idPrefix: string, field: string, slot: typeof options.invertY) {
  return optionChoices([
    radioChoice(`${idPrefix}Off`, "OFF", stateEquals(slot, false), `frontend.options.${field}.off`),
    radioChoice(`${idPrefix}On`, "ON", stateEquals(slot, true), `frontend.options.${field}.on`),
  ]);
}

/// A slider over `[min, max]` bound to an option's working copy.
function rangeSlider(
  id: string,
  labelledBy: string,
  bind: typeof options.mouseSensitivity,
  min: number,
  max: number,
  step: number,
  decimalPlaces: number,
) {
  return optionValue(
    Slider({
      id,
      labelledBy,
      bind,
      min,
      max,
      step,
      valueDisplay: { min, max, decimalPlaces },
      capturesNav: ["nav.left", "nav.right"],
    }),
  );
}

function optionLabel(id: string, label: string, note?: string) {
  return VStack({ gap: 2, align: "start", role: "group" }, [
    Text({ id, content: label, fontSize: 14 }),
    ...(note === undefined ? [] : [Text({ content: note, fontSize: 11, color: COLOR_MUTED })]),
  ]);
}

function optionValue(control: ReturnType<typeof Button> | ReturnType<typeof Slider>) {
  return HStack({ align: "center" }, [control]);
}

function optionChoices(choices: ReturnType<typeof Button>[]) {
  return optionValue(HStack({ gap: 6, align: "stretch" }, choices));
}

// The options screen is tabbed. The selected tab is a presentation-local cell
// (`ui.createLocalState`), never a player option: it lives only while the tree
// is retained, so each open starts on CONTROLS. The pattern is the tabs demo's
// (`tabs-demo.ts`): each tab button's predicate drives both the styleRanges
// highlight and the a11y `selected` state, its `onPress` names a `cellWrite`
// reaction, and `Switch` hides every panel but the active one.
const optionsTabState = ui.createLocalState({ tab: "controls" });
const optionsTab = optionsTabState.cells.tab;

type OptionsTabKey = "controls" | "graphics" | "accessibility";

const OPTIONS_TABS: ReadonlyArray<{ key: OptionsTabKey; id: string; label: string }> = [
  { key: "controls", id: "optionsTabControls", label: "CONTROLS" },
  { key: "graphics", id: "optionsTabGraphics", label: "GRAPHICS" },
  { key: "accessibility", id: "optionsTabAccessibility", label: "ACCESSIBILITY" },
];

const optionsTabReactions: NamedReactionDescriptor[] = OPTIONS_TABS.map(({ key }) =>
  defineReaction(`frontend.options.tab.${key}`, optionsTab.set(key)),
);

function optionsTabButton(tab: (typeof OPTIONS_TABS)[number], index: number) {
  const active = optionsTab.is(tab.key);
  return Button({
    id: tab.id,
    label: tab.label,
    role: "tab",
    bind: active,
    selected: active,
    styleRanges: {
      max: 1,
      entries: [{ upTo: 0, color: COLOR_INACTIVE }, { color: COLOR_ACCENT }],
    },
    onPress: optionsTabReactions[index],
  });
}

function optionsPanel(id: string, children: WidgetDescriptor[]) {
  return VStack({ id, gap: 10, align: "stretch", role: "group" }, children);
}

// Each panel's grid is its own spatial focus group nested in the screen's
// linear group: Up from the top row leaves for the tab strip, Down from the
// bottom row reaches BACK, and returning lands on the control last focused.
const controlsPanel = optionsPanel("optionsPanelControls", [
  Grid({ gap: 12, align: "stretch", cols: 2, focus: { policy: "spatial" } }, [
    optionLabel("optionsRebindLabel", "BINDINGS", "Keyboard, mouse and gamepad"),
    optionValue(Button({ id: "optionsRebind", label: "CONTROLS", onPress: OPEN_CONTROLS_ACTION })),
    optionLabel("optionsMouseSensitivityLabel", "MOUSE SENSITIVITY"),
    optionValue(
      Slider({
        id: "optionsMouseSensitivity",
        labelledBy: "optionsMouseSensitivityLabel",
        bind: options.mouseSensitivity,
        min: 0.0005,
        max: 0.01,
        step: 0.0005,
        valueDisplay: { min: 1, max: 100, suffix: "%", decimalPlaces: 0 },
        capturesNav: ["nav.left", "nav.right"],
      }),
    ),
    optionLabel("optionsInvertYLabel", "INVERT Y"),
    optionChoices([
      radioChoice(
        "optionsInvertYOff",
        "OFF",
        stateEquals(options.invertY, false),
        "frontend.options.invertY.off",
      ),
      radioChoice(
        "optionsInvertYOn",
        "ON",
        stateEquals(options.invertY, true),
        "frontend.options.invertY.on",
      ),
    ]),
    optionLabel("optionsViewFeelScaleLabel", "VIEW FEEL"),
    optionValue(
      Slider({
        id: "optionsViewFeelScale",
        labelledBy: "optionsViewFeelScaleLabel",
        bind: options.viewFeelScale,
        min: 0,
        max: 1,
        step: 0.1,
        capturesNav: ["nav.left", "nav.right"],
      }),
    ),
    optionLabel("optionsCrouchModeLabel", "CROUCH MODE"),
    optionChoices([
      radioChoice(
        "optionsCrouchHold",
        "HOLD",
        stateEquals(options.crouchMode, "hold"),
        "frontend.options.crouchMode.hold",
      ),
      radioChoice(
        "optionsCrouchToggle",
        "TOGGLE",
        stateEquals(options.crouchMode, "toggle"),
        "frontend.options.crouchMode.toggle",
      ),
    ]),
    optionLabel("optionsSprintModeLabel", "SPRINT MODE"),
    optionChoices([
      radioChoice(
        "optionsSprintHold",
        "HOLD",
        stateEquals(options.sprintMode, "hold"),
        "frontend.options.sprintMode.hold",
      ),
      radioChoice(
        "optionsSprintToggle",
        "TOGGLE",
        stateEquals(options.sprintMode, "toggle"),
        "frontend.options.sprintMode.toggle",
      ),
    ]),
    optionLabel("optionsGamepadLookSensitivityLabel", "GAMEPAD LOOK SPEED"),
    rangeSlider(
      "optionsGamepadLookSensitivity",
      "optionsGamepadLookSensitivityLabel",
      options.gamepadLookSensitivity,
      0.5,
      8,
      0.25,
      2,
    ),
    optionLabel("optionsGamepadLookDeadZoneLabel", "GAMEPAD LOOK DEAD ZONE"),
    rangeSlider(
      "optionsGamepadLookDeadZone",
      "optionsGamepadLookDeadZoneLabel",
      options.gamepadLookDeadZone,
      0,
      0.5,
      0.05,
      2,
    ),
    optionLabel("optionsGamepadInvertYLabel", "GAMEPAD INVERT Y"),
    offOnChoices("optionsGamepadInvertY", "gamepadInvertY", options.gamepadInvertY),
    optionLabel("optionsSwapConfirmCancelLabel", "SWAP CONFIRM / CANCEL", "For pads with confirm on the right"),
    offOnChoices("optionsSwapConfirmCancel", "swapConfirmCancel", options.swapConfirmCancel),
  ]),
]);

function displayModeControls(value: (typeof WINDOW_MODE_CHOICES)[number]["value"]) {
  const disabled = value === "borderless";
  const color: [number, number, number, number] = [1, 1, 1, disabled ? 0.8 : 1];
  const visibleWhen = stateEquals(options.windowMode, value);
  const suffix = value === "windowed" ? "" : value === "exclusive" ? "Exclusive" : "Borderless";
  const button = (id: string, label: string, op: "previous" | "next" | "apply", inactive = false) => Button({
    id: `${id}${suffix}`,
    label,
    onPress: displayModeAction(op),
    disabled: disabled || inactive,
    bind: visibleWhen,
    styleRanges: { max: 1, entries: [{ color: inactive ? [1, 1, 1, 0.8] : color }] },
  });
  return VStack({ gap: 6, align: "start", visibleWhen }, [
    HStack({ gap: 4, align: "center" }, [
      button("displayModePrev", "<", "previous"),
      Text({ content: "", color, bind: bindState(window.displayModeWidth, { format: "{}x", decimalPlaces: 0 }) }),
      Text({ content: "", color, bind: bindState(window.displayModeHeight, { decimalPlaces: 0 }) }),
      Text({ content: "", color, bind: bindState(window.displayModeRefreshHz, { format: " @ {} Hz", decimalPlaces: 0 }) }),
      button("displayModeNext", ">", "next"),
    ]),
    ...(disabled ? [button("displayModeApply", "APPLY RESOLUTION", "apply")] : [
      VStack({ visibleWhen: stateEquals(window.displayModeCanApply, true) }, [
        button("displayModeApply", "APPLY RESOLUTION", "apply"),
      ]),
      VStack({ visibleWhen: stateEquals(window.displayModeCanApply, false) }, [
        button("displayModeApplyDisabled", "APPLY RESOLUTION", "apply", true),
      ]),
    ]),
  ]);
}

const graphicsPanel = optionsPanel("optionsPanelGraphics", [
  Grid({ gap: 12, align: "stretch", cols: 2, focus: { policy: "spatial" } }, [
    optionLabel("optionsWindowModeLabel", "WINDOW MODE"),
    optionChoices(WINDOW_MODE_CHOICES.map(({ value, id, label }) =>
      radioChoice(id, label, stateEquals(options.windowMode, value), `frontend.options.windowMode.${value}`),
    )),
    VStack({ id: "optionsDisplayModeLabel", align: "start" }, WINDOW_MODE_CHOICES.map(({ value }) => Text({
      content: "DISPLAY MODE",
      fontSize: 14,
      color: [1, 1, 1, value === "borderless" ? 0.8 : 1],
      visibleWhen: stateEquals(options.windowMode, value),
    }))),
    optionValue(VStack({ align: "start" }, WINDOW_MODE_CHOICES.map(({ value }) => displayModeControls(value)))),
    optionLabel("optionsShadowQualityLabel", "SHADOW QUALITY", "Applies after reload"),
    optionChoices([
      radioChoice(
        "optionsShadowLow",
        "LOW",
        stateEquals(options.shadowQuality, "low"),
        "frontend.options.shadowQuality.low",
      ),
      radioChoice(
        "optionsShadowMedium",
        "MEDIUM",
        stateEquals(options.shadowQuality, "medium"),
        "frontend.options.shadowQuality.medium",
      ),
      radioChoice(
        "optionsShadowHigh",
        "HIGH",
        stateEquals(options.shadowQuality, "high"),
        "frontend.options.shadowQuality.high",
      ),
    ]),
    optionLabel("optionsFogQualityLabel", "FOG QUALITY"),
    optionChoices([
      radioChoice(
        "optionsFogLow",
        "LOW",
        stateEquals(options.fogQuality, "low"),
        "frontend.options.fogQuality.low",
      ),
      radioChoice(
        "optionsFogMedium",
        "MEDIUM",
        stateEquals(options.fogQuality, "medium"),
        "frontend.options.fogQuality.medium",
      ),
      radioChoice(
        "optionsFogHigh",
        "HIGH",
        stateEquals(options.fogQuality, "high"),
        "frontend.options.fogQuality.high",
      ),
    ]),
    optionLabel("optionsSurfaceDepthQualityLabel", "SURFACE DEPTH"),
    optionChoices([
      radioChoice(
        "optionsSurfaceDepthOff",
        "OFF",
        stateEquals(options.surfaceDepthQuality, "off"),
        "frontend.options.surfaceDepthQuality.off",
      ),
      radioChoice(
        "optionsSurfaceDepthOn",
        "ON",
        stateEquals(options.surfaceDepthQuality, "on"),
        "frontend.options.surfaceDepthQuality.on",
      ),
    ]),
    optionLabel("optionsRenderResolutionLabel", "RENDER RESOLUTION"),
    optionChoices(
      RENDER_RESOLUTION_CHOICES.map(({ value, id, label }) =>
        radioChoice(
          id,
          label,
          stateEquals(options.renderResolution, value),
          `frontend.options.renderResolution.${value}`,
        ),
      ),
    ),
  ]),
]);

/// A toggle's one control: a button showing the field's current value, named by
/// its label on the left. Pressing it fires the field's reserved action, the
/// same write the engine accessibility panel's control makes; the button keeps
/// its id as its text changes, so focus stays on it.
function valueButton(
  id: string,
  labelledBy: string,
  field: "reduceMotion" | "flashLimiter" | "monoAudio",
  valueText: ValueTextCase[],
) {
  return optionValue(
    Button({ id, labelledBy, onPress: accessibilityAction(field, "cycle"), valueText }),
  );
}

/// ON while the resolved `accessibility.<field>` slot is true, else OFF.
function onOff(on: Predicate): ValueTextCase[] {
  return [{ when: [on], text: "ON" }, { text: "OFF" }];
}

const followsSystem = stateEquals(accessibility.reduceMotionFollowsSystem, true);
const motionReduced = stateEquals(accessibility.reduceMotion, true);

const accessibilityPanel = optionsPanel("optionsPanelAccessibility", [
  Grid({ gap: 12, align: "stretch", cols: 2, focus: { policy: "spatial" } }, [
    optionLabel("optionsReduceMotionLabel", "REDUCE MOTION"),
    valueButton("optionsReduceMotion", "optionsReduceMotionLabel", "reduceMotion", [
      { when: [followsSystem, motionReduced], text: "SYSTEM (ON)" },
      { when: [followsSystem], text: "SYSTEM (OFF)" },
      { when: [motionReduced], text: "ON" },
      { text: "OFF" },
    ]),
    optionLabel("optionsScreenShakeScaleLabel", "SCREEN SHAKE"),
    unitSlider(
      "optionsScreenShakeScale",
      "optionsScreenShakeScaleLabel",
      options.screenShakeScale,
      0.1,
    ),
    optionLabel("optionsA11yViewFeelScaleLabel", "VIEW FEEL"),
    unitSlider(
      "optionsA11yViewFeelScale",
      "optionsA11yViewFeelScaleLabel",
      options.viewFeelScale,
      0.1,
    ),
    optionLabel("optionsHoldTimingScaleLabel", "HOLD TIMING", "Longer tap and hold windows"),
    rangeSlider(
      "optionsHoldTimingScale",
      "optionsHoldTimingScaleLabel",
      options.holdTimingScale,
      1,
      3,
      0.25,
      2,
    ),
    optionLabel("optionsFlashLimiterLabel", "FLASH LIMITER"),
    valueButton(
      "optionsFlashLimiter",
      "optionsFlashLimiterLabel",
      "flashLimiter",
      onOff(stateEquals(accessibility.flashLimiter, true)),
    ),
    optionLabel("optionsMasterVolumeLabel", "MASTER VOLUME"),
    unitSlider("optionsMasterVolume", "optionsMasterVolumeLabel", options.masterVolume, 0.05),
    optionLabel("optionsSfxVolumeLabel", "SFX VOLUME"),
    unitSlider("optionsSfxVolume", "optionsSfxVolumeLabel", options.sfxVolume, 0.05),
    optionLabel("optionsMusicVolumeLabel", "MUSIC VOLUME"),
    unitSlider("optionsMusicVolume", "optionsMusicVolumeLabel", options.musicVolume, 0.05),
    optionLabel("optionsUiVolumeLabel", "UI VOLUME"),
    unitSlider("optionsUiVolume", "optionsUiVolumeLabel", options.uiVolume, 0.05),
    optionLabel("optionsMonoAudioLabel", "MONO AUDIO"),
    valueButton(
      "optionsMonoAudio",
      "optionsMonoAudioLabel",
      "monoAudio",
      onOff(stateEquals(accessibility.monoAudio, true)),
    ),
  ]),
]);

export const optionsMenu = defineUiTree({
  name: OPTIONS_MENU_NAME,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: OPTIONS_TABS[0].id,
      accessibleName: "Player options",
      role: "group",
    },
    // The screen is one linear group holding three stops: the tab strip (a
    // nested horizontal group that wraps), the visible panel's grid (a nested
    // spatial group), and BACK. Down from a tab enters the panel; the bumpers
    // switch tabs from anywhere. Hidden panels drop out of the focus export.
    // Closing a tree pushed above this one returns focus to the last-focused
    // control (restore is on by default).
    VStack(
      {
        localState: optionsTabState.scope,
        gap: 20,
        padding: 24,
        align: "stretch",
        width: 640,
        fill: COLOR_PANEL,
        focus: { policy: "linear", wrap: true },
      },
      [
        Text({ content: "OPTIONS", fontSize: 24, color: COLOR_ACCENT }),
        HStack(
          { gap: 6, align: "stretch", role: "tablist", focus: { policy: "linear", wrap: true } },
          OPTIONS_TABS.map(optionsTabButton),
        ),
        VStack(
          { align: "stretch" },
          Switch(optionsTab, {
            controls: controlsPanel,
            graphics: graphicsPanel,
            accessibility: accessibilityPanel,
          }),
        ),
        Button({ id: "optionsBack", label: "BACK", onPress: CLOSE_DIALOG_ACTION }),
        promptRow(),
      ],
    ),
  ),
});

export const frontendReactions: NamedReactionDescriptor[] = [
  openPlay,
  openOptions,
  askExit,
  askQuit,
  ...frontendStartReactions,
  ...optionReactions,
  ...optionsTabReactions,
];
