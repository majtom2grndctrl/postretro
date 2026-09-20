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
  Grid,
  HStack,
  Slider,
  Text,
  Tree,
  VStack,
  defineUiTree,
  getGameState,
  loadLevel,
  openMenu,
  stateEquals,
  updateState,
  type Predicate,
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

function section(title: string, entries: ModMapEntry[]) {
  return VStack({ gap: 6, align: "stretch" }, [
    Text({ content: title, fontSize: 16 }),
    ...entries.map(levelButton),
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
        HStack({ gap: 18, align: "start" }, [
          section("Recommended", mapsTagged("recommended")),
          section("Development Tests", mapsTagged("test")),
        ]),
        Button({
          id: "levelSelectBack",
          label: "BACK",
          onPress: CLOSE_DIALOG_ACTION,
        }),
      ],
    ),
  ),
});

const openPlay = defineReaction("frontend.openPlay", openMenu(LEVEL_SELECT_MENU_NAME));
const openOptions = defineReaction("frontend.openOptions", openMenu(OPTIONS_MENU_NAME));

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
        Button({ id: "frontendExit", label: "EXIT", onPress: EXIT_TO_DESKTOP_ACTION }),
      ],
    ),
  ),
});

const options = getGameState().options;

const optionReactions: NamedReactionDescriptor[] = [
  defineReaction("frontend.options.invertY.off", updateState(options.invertY, false)),
  defineReaction("frontend.options.invertY.on", updateState(options.invertY, true)),
  defineReaction("frontend.options.crouchMode.hold", updateState(options.crouchMode, "hold")),
  defineReaction("frontend.options.crouchMode.toggle", updateState(options.crouchMode, "toggle")),
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
];

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

export const optionsMenu = defineUiTree({
  name: OPTIONS_MENU_NAME,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: "optionsMouseSensitivity",
      accessibleName: "Player options",
      role: "group",
    },
    VStack(
      {
        gap: 20,
        padding: 24,
        align: "stretch",
        width: 640,
        fill: COLOR_PANEL,
        focus: { policy: "linear", wrap: true },
      },
      [
        Text({ content: "OPTIONS", fontSize: 24, color: COLOR_ACCENT }),
        VStack({ gap: 10, align: "stretch", role: "group" }, [
          Text({ content: "CONTROLS", fontSize: 12, color: COLOR_MUTED }),
          Grid({ gap: 12, align: "stretch", cols: 2 }, [
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
          ]),
        ]),
        VStack({ gap: 10, align: "stretch", role: "group" }, [
          Text({ content: "GRAPHICS", fontSize: 12, color: COLOR_MUTED }),
          Grid({ gap: 12, align: "stretch", cols: 2 }, [
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
          ]),
        ]),
        Button({ id: "optionsBack", label: "BACK", onPress: CLOSE_DIALOG_ACTION }),
      ],
    ),
  ),
});

export const frontendReactions: NamedReactionDescriptor[] = [
  openPlay,
  openOptions,
  ...frontendStartReactions,
  ...optionReactions,
];
