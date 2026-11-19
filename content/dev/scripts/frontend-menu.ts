import {
  defineMapCatalog,
  defineReaction,
  type CommandId,
  type ModMapEntry,
  type NamedReactionDescriptor,
} from "postretro";
import {
  Button,
  CLOSE_DIALOG_ACTION,
  appendText,
  backspaceText,
  EXIT_TO_DESKTOP_ACTION,
  Glyph,
  Grid,
  HStack,
  OPEN_CONTROLS_ACTION,
  QUIT_TO_MENU_ACTION,
  Slider,
  Spacer,
  Switch,
  Text,
  Tree,
  VStack,
  accessibilityAction,
  bindState,
  clearText,
  closeDialog,
  continueRun,
  createCharacter,
  deleteCharacter,
  deleteRun,
  displayModeAction,
  defineUiTree,
  endSession,
  getGameState,
  hostSession,
  joinSession,
  KEYBOARD_TREE,
  leaveSession,
  loadLevel,
  newRun,
  openMenu,
  openTextEntry,
  selectCharacter,
  stateCompare,
  stateEquals,
  ui,
  updateState,
  type Predicate,
  type ValueTextCase,
  type WidgetDescriptor,
} from "postretro/ui";

// Tests bundle this module standalone (`menu_tests`, `window_mode_sdk_tests`),
// so it takes no relative imports; `loading-screens.ts`, `pause-menu.ts` and
// `frontend-saves.ts` import from here. Every frontend reaction lives here, in
// `frontendReactions`, so a standalone bundle can press any of them;
// `frontend-saves.ts` only lays out the character, run and lobby screens.

/// The loading tree `loading-screens.ts` registers for catalog map `mapId`.
export function loadingTreeName(mapId: string): string {
  return `dev.loading.${mapId}`;
}

const TITLE_MENU_NAME = "frontend.menuTree";
const LEVEL_SELECT_MENU_NAME = "frontend.devLevelSelect";
const OPTIONS_MENU_NAME = "frontend.options";
/** The save screens `frontend-saves.ts` registers under these names. */
export const CHARACTERS_MENU_NAME = "frontend.characters";
export const RUNS_MENU_NAME = "frontend.runs";
export const LOBBY_MENU_NAME = "frontend.lobby";

type Color = [number, number, number, number];

export const COLOR_ACCENT: Color = [0.12, 0.72, 0.4, 1.0];
/// An unselected choice: dimmer than the selected one, still clearly there.
const COLOR_UNSELECTED: Color = [0.5, 0.57, 0.6, 1.0];
export const COLOR_MUTED: Color = [0.58, 0.68, 0.72, 1.0];
const COLOR_PANEL: Color = [0.018, 0.026, 0.039, 0.94];

// --- Visual grammar ------------------------------------------------------------
// Every frontend screen is one layer that owns the screen (`hideBelow`), laid
// out the same way, so what a thing is shows in how it looks:
//
// - Information is text and only text: a screen title, callouts (a tinted box
//   under a stripe: green for status, amber for a notice), body copy in
//   sentence case, hints muted. Nothing informational sits in a filled box.
// - A control is CAPS in a filled box. The fill says what pressing it does:
//   green is the screen's one primary action, slate a secondary one, red
//   ends a session or quits, and the faint box in the footer closes this layer.
// - An editable value is one row: its caption, its value in a dark well, and
//   an EDIT box beside it.
// - The footer, under a rule, holds the layer's only way back, bottom left
//   beside its pad glyph, and the input legend on the right. A dialog's
//   footer also holds its one action, on the right.
//
// The engine draws a button as its label alone, so a box is a container fill
// around the button, and a label color is a one-band `styleRanges` over any
// predicate: one band colors both of its values.

const LABEL_WHITE: Color = [1.0, 1.0, 1.0, 1.0];
export const COLOR_BODY: Color = [0.84, 0.9, 0.92, 1.0];
const COLOR_RULE: Color = [1.0, 1.0, 1.0, 0.1];
const FILL_WELL: Color = [0.0, 0.0, 0.0, 0.5];

type ActionKind = "primary" | "secondary" | "session" | "nav";

const ACTION_FILL: Record<ActionKind, Color> = {
  primary: [0.02, 0.2, 0.1, 1.0],
  secondary: [0.045, 0.075, 0.13, 1.0],
  session: [0.22, 0.03, 0.035, 1.0],
  // A faint box with a muted label: it leaves rather than acts.
  nav: [1.0, 1.0, 1.0, 0.08],
};
const ACTION_LABEL: Record<ActionKind, Color> = {
  primary: LABEL_WHITE,
  secondary: LABEL_WHITE,
  session: LABEL_WHITE,
  nav: [0.74, 0.82, 0.86, 1.0],
};

type ButtonProps = Parameters<typeof Button>[0];

/// The source for a constant label color: any predicate serves.
const STYLE_SOURCE = stateEquals(getGameState().session.connection, "offline");

/// A pressable control: its button in a filled box. A `visibleWhen` moves to
/// the box, so the box hides with its button. The pause menu shares it.
export function action(kind: ActionKind, props: ButtonProps, padding = 7): WidgetDescriptor {
  const { visibleWhen, ...rest } = props;
  const label = ACTION_LABEL[kind];
  const styled: ButtonProps =
    label === LABEL_WHITE || rest.bind !== undefined
      ? (rest as ButtonProps)
      : ({ ...rest, bind: STYLE_SOURCE, styleRanges: { max: 1, entries: [{ color: label }] } } as ButtonProps);
  return VStack(
    { fill: ACTION_FILL[kind], padding, align: "stretch", ...(visibleWhen === undefined ? {} : { visibleWhen }) },
    [Button(styled)],
  );
}

/// A rule across the panel.
function rule(): WidgetDescriptor {
  return VStack({ fill: COLOR_RULE, padding: 0.5 }, []);
}

/// The input legend: what confirm does. Back's glyph sits beside the back box.
function legend(): WidgetDescriptor[] {
  return [Glyph({ command: "nav_confirm" }), Text({ content: "Select", fontSize: 13, color: COLOR_MUTED })];
}

/// A layer's footer: its one way back on the left, with the glyph that also
/// presses it, and `right` (the legend, or a dialog's action) on the right.
export function footer(close: WidgetDescriptor[], right: WidgetDescriptor[] = legend()): WidgetDescriptor {
  return VStack({ gap: 12, align: "stretch" }, [
    rule(),
    HStack({ gap: 8, align: "center" }, [...close, Spacer({ flexGrow: 1 }), ...right]),
  ]);
}

/// The back box that closes the layer it sits in.
export function backAction(id: string, label = "BACK"): WidgetDescriptor[] {
  return [action("nav", { id, label, onPress: CLOSE_DIALOG_ACTION }), Glyph({ command: "nav_cancel" })];
}

/// A screen's title: information, never pressable.
export function title(content: string, fontSize = 24): WidgetDescriptor {
  return Text({ content, fontSize, color: COLOR_ACCENT });
}

/// Body copy, sentence case.
export function body(content: string, visibleWhen?: Predicate): WidgetDescriptor {
  return Text({ content, fontSize: 14, color: COLOR_BODY, ...(visibleWhen === undefined ? {} : { visibleWhen }) });
}

/// A hint: what to do, or what follows. Sentence case, muted.
export function hint(content: string): WidgetDescriptor {
  return Text({ content, fontSize: 13, color: COLOR_MUTED });
}

type Tone = "status" | "notice";
export const COLOR_NOTICE: Color = [0.98, 0.68, 0.22, 1.0];
const TONE_STRIPE: Record<Tone, Color> = { status: COLOR_ACCENT, notice: COLOR_NOTICE };
const TONE_FILL: Record<Tone, Color> = { status: [0.0, 0.0, 0.0, 0.28], notice: [0.12, 0.05, 0.0, 0.4] };

/// A callout: a tinted box under a colored stripe, holding a headline and its
/// details. Status is green, a notice amber. Its only control, if any, is the
/// notice's answer, boxed like every control.
export function callout(tone: Tone, children: WidgetDescriptor[], visibleWhen?: Predicate): WidgetDescriptor {
  return VStack(
    { gap: 0, align: "stretch", fill: TONE_FILL[tone], ...(visibleWhen === undefined ? {} : { visibleWhen }) },
    [VStack({ fill: TONE_STRIPE[tone], padding: 1.5 }, []), VStack({ gap: 5, padding: 10, align: "stretch" }, children)],
  );
}

export function headline(tone: Tone, content: string): WidgetDescriptor {
  return Text({ content, fontSize: 16, color: TONE_STRIPE[tone] });
}

/// A frontend layer's panel: title, body, footer, in one linear focus group.
export function panel(
  props: { width?: number; wrap?: boolean; localState?: NonNullable<Parameters<typeof VStack>[0]>["localState"] },
  children: WidgetDescriptor[],
): WidgetDescriptor {
  return VStack(
    {
      gap: 14,
      padding: 24,
      align: "stretch",
      fill: COLOR_PANEL,
      focus: { policy: "linear", wrap: props.wrap ?? true },
      ...(props.width === undefined ? {} : { width: props.width }),
      ...(props.localState === undefined ? {} : { localState: props.localState }),
    },
    children,
  );
}

export const mapCatalog = defineMapCatalog([
  {
    id: "campaign-test",
    path: "maps/campaign-test.prl",
    name: "Moody Vibe Test",
    tags: ["campaign", "recommended"],
    loadingTree: loadingTreeName("campaign-test"),
  },
  {
    id: "kinematic-platform",
    path: "maps/kinematic-platform.prl",
    name: "Moving Platforms Test",
    tags: ["platform", "test"],
    loadingTree: loadingTreeName("kinematic-platform"),
  },
  {
    id: "movement-feel",
    path: "maps/movement-feel.prl",
    name: "Combat Arena Test",
    tags: ["combat", "movement", "recommended"],
    loadingTree: loadingTreeName("movement-feel"),
  },
  {
    id: "stress-warren-hallway-inspection",
    path: "maps/stress-warren-hallway-inspection.prl",
    name: "Stress Test",
    tags: ["stress", "test"],
    loadingTree: loadingTreeName("stress-warren-hallway-inspection"),
  },
  {
    id: "occlusion-test",
    path: "maps/occlusion-test.prl",
    name: "Occlusion Test",
    tags: ["occlusion", "lighting", "test"],
    loadingTree: loadingTreeName("occlusion-test"),
  },
  {
    id: "combat-demo",
    path: "maps/combat-demo.prl",
    name: "Combat + Emissive Test",
    tags: ["combat", "emissive", "recommended"],
    // Also the frontend backdrop, so its loading screen shows at every boot.
    loadingTree: loadingTreeName("combat-demo"),
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
  return action("secondary", {
    id: `start-${entry.id}`,
    label: entry.name,
    onPress: startReactionName(entry),
  });
}

// A column of level buttons that scrolls once it outgrows the screen. Scroll
// opens no focus group, so both columns' buttons share the level select's
// spatial group: Left and Right cross between columns.
function section(caption: string, entries: ModMapEntry[]) {
  return VStack({ gap: 8, align: "stretch" }, [
    Text({ content: caption, fontSize: 14, color: COLOR_MUTED }),
    VStack({ gap: 8, align: "stretch", scroll: { maxHeight: 360 } }, entries.map(levelButton)),
  ]);
}

function mapsTagged(tag: string): ModMapEntry[] {
  return mapCatalog.filter((entry) => hasTag(entry, tag));
}

export const devLevelSelectMenu = defineUiTree({
  name: LEVEL_SELECT_MENU_NAME,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: `start-${mapCatalog[0].id}`,
      accessibleName: "Dev level select",
      role: "group",
    },
    panel({}, [
      title("PLAY"),
      // A host's players follow it into the level it starts.
      VStack({ align: "stretch", visibleWhen: stateEquals(getGameState().session.hosting, true) }, [
        body("Players waiting at your menu join you in this level."),
      ]),
      HStack({ gap: 18, align: "start", focus: { policy: "spatial" } }, [
        section("Recommended", mapsTagged("recommended")),
        section("Development tests", mapsTagged("test")),
      ]),
      footer(backAction("levelSelectBack")),
    ]),
  ),
});

// Dev EXIT and QUIT confirmations: each opens a dialog whose initial focus is
// the safe choice, so a second confirm pressed on the next frame cancels.
const EXIT_CONFIRM_NAME = "dev.exitConfirm";
const QUIT_CONFIRM_NAME = "dev.quitConfirm";

// A dialog is a layer too: it hides what opened it. Its footer holds its way
// back and its one action, which ends something, so it is red. `detail` is
// information under the prompt: what the action ends.
export function confirmDialog(
  name: string,
  prompt: string,
  label: string,
  onPress: string,
  idPrefix: string,
  detail: WidgetDescriptor[] = [],
) {
  return defineUiTree({
    name,
    hideBelow: true,
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
      panel({ width: 380, wrap: false }, [
        title(prompt, 22),
        ...detail,
        footer(backAction(`${idPrefix}Cancel`, "CANCEL"), [action("session", { id: `${idPrefix}Confirm`, label, onPress })]),
      ]),
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

// PLAY starts the save flow: characters, then runs. The level picker stays a
// dev path of its own.
const openPlay = defineReaction("frontend.openPlay", openMenu(CHARACTERS_MENU_NAME));
/** Opens the dev level picker: the title's and the lobby's LEVEL SELECT. */
export const openLevelSelect = defineReaction("frontend.openLevelSelect", openMenu(LEVEL_SELECT_MENU_NAME));

// --- Host and Join ---------------------------------------------------------
// The engine owns entry, admission and the session facts; these menus only
// show them. Each end cause, connection phase and wait reason is data the
// engine publishes; the wording is the mod's.
//
// Each menu shows one state at a time, keyed on the session facts: the form
// while offline, and only the status and the action that ends it while
// connecting, waiting or hosting. That action is red and names what it ends,
// so it never reads as the footer's BACK, which only closes the menu. The
// status group sits first in tree order: when the focused button hides with
// its state, focus falls back to the first visible stop, the new state's own.

const HOST_MENU_NAME = "frontend.host";
const JOIN_MENU_NAME = "frontend.join";
const { session } = getGameState();

/// The headline for each `session.endCause` value: why the last session or
/// attempt ended. ASCII only: it renders in every bundled typeface.
export const END_CAUSE_NOTICES = {
  sessionEnded: "THE HOST ENDED THE SESSION",
  hostLost: "LOST CONNECTION TO THE HOST",
  connectFailed: "COULD NOT REACH THE HOST",
  refusedMod: "THE HOST IS PLAYING A DIFFERENT MOD",
  refusedPassword: "PASSWORD REFUSED",
  badAddress: "NOT A HOST ADDRESS",
  hostFailed: "COULD NOT START HOSTING",
} as const;

type EndCause = keyof typeof END_CAUSE_NOTICES;

/// What to do about a notice, where there is something to do.
const END_CAUSE_HINTS: Partial<Record<EndCause, string>> = {
  connectFailed: "Check the address, and that the host runs your build.",
  refusedMod: "Both players need the same mod.",
  refusedPassword: "Enter the host's password, then join again.",
  badAddress: "Use IPv4 digits, like 192.168.1.20. Names do not work.",
  hostFailed: "Another program may be using the port.",
};

/// A cause's callout: its headline, its hint, and the press that answers it.
export function endCauseNotice(cause: EndCause, answer?: WidgetDescriptor): WidgetDescriptor {
  const fix = END_CAUSE_HINTS[cause];
  return callout(
    "notice",
    [
      headline("notice", END_CAUSE_NOTICES[cause]),
      ...(fix === undefined ? [] : [hint(fix)]),
      ...(answer === undefined ? [] : [answer]),
    ],
    stateEquals(session.endCause, cause),
  );
}

function endCauseNotices(causes: readonly EndCause[]): WidgetDescriptor[] {
  return causes.map((cause) => endCauseNotice(cause));
}

const ALL_END_CAUSES = Object.keys(END_CAUSE_NOTICES) as EndCause[];
/// Every way a join, or a session joined, ends.
const JOIN_END_CAUSES = ALL_END_CAUSES.filter((cause) => cause !== "hostFailed");

// --- Text fields and the on-screen keyboard ---------------------------------
// The dev mod shadows the engine keyboard (`core/ui/keyboard.json`, same
// registry name) with one that says what it is for and shows what is typed.
// Each field's opener first tells it which layout, title, hint and masking to
// use, through presentation cells; the cells live only while the keyboard is
// up, so an untargeted push gets the full layout with no title. The preview
// starts masked: only a field that says so shows its text.

const keyboardState = ui.createLocalState<{
  layout: string;
  masked: boolean;
  title: string;
  hint: string;
}>({ layout: "letters", masked: true, title: "", hint: "" });
const kb = keyboardState.cells;

type KeyboardLayout = "letters" | "address";

type KeyboardField = { layout: KeyboardLayout; masked: boolean; title: string; hint: string };

/// Open the keyboard for one field: the cells first, so the push shows them.
function editFieldSteps(target: typeof session.joinPassword, field: KeyboardField) {
  return [
    kb.layout.set(field.layout),
    kb.masked.set(field.masked),
    kb.title.set(field.title),
    kb.hint.set(field.hint),
    openTextEntry(undefined, target),
  ];
}

const JOIN_ADDRESS_FIELD: KeyboardField = {
  layout: "address",
  masked: false,
  title: "HOST ADDRESS",
  hint: "IPv4, like 192.168.1.20. Add :port only if the host set one.",
};
const JOIN_PASSWORD_FIELD: KeyboardField = {
  layout: "letters",
  masked: true,
  title: "PASSWORD",
  hint: "The host's password, exactly. Empty for none.",
};
const HOST_PASSWORD_FIELD: KeyboardField = {
  layout: "letters",
  masked: true,
  title: "PASSWORD FOR YOUR GAME",
  hint: "Players type this to join. Empty for an open game.",
};

const editJoinAddress = defineReaction(
  "frontend.editJoinAddress",
  editFieldSteps(session.joinAddress, JOIN_ADDRESS_FIELD),
);
const editPassword = defineReaction(
  "frontend.editPassword",
  editFieldSteps(session.joinPassword, JOIN_PASSWORD_FIELD),
);
const editHostPassword = defineReaction(
  "frontend.editHostPassword",
  editFieldSteps(session.hostPassword, HOST_PASSWORD_FIELD),
);

const COMMIT_TEXT_ENTRY_ACTION = "ui.commitTextEntry";

/// The key reaction name for one character.
function keyReaction(ch: string): string {
  const named: Record<string, string> = { ".": "dot", ":": "colon", "-": "dash", _: "underscore" };
  return `kbAppend_${named[ch] ?? ch}`;
}

/// One character key, labelled with exactly what it types: the board is
/// lowercase, so its letters are too.
function charKey(idPrefix: string, ch: string, padding = 5): WidgetDescriptor {
  const name = keyReaction(ch).slice("kbAppend_".length);
  return action("secondary", { id: `${idPrefix}_${name}`, label: ch, onPress: keyReaction(ch) }, padding);
}

/// Digits and address punctuation, phone-pad order: an address is typed in a
/// few moves from the pad's first key.
const ADDRESS_PAD = ["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", ":"];
const LETTER_ROWS = ["1234567890", "qwertyuiop", "asdfghjkl.", "zxcvbnm:-_"];

/// An editing key with the pad shortcut that presses it beside it. The engine
/// shortcuts find these keys by id (`key_space`, `key_backspace`, `key_done`).
function editKey(kind: ActionKind, id: string, label: string, onPress: string, command: CommandId): WidgetDescriptor {
  return HStack({ gap: 6, align: "center" }, [action(kind, { id, label, onPress }), Glyph({ command })]);
}

// The keyboard is its field's own layer: the title names the field, the
// preview well looks like the field's value well, the editing keys sit under
// the board with DONE as its primary action, and CANCEL is the footer's way
// back, as BACK is on every menu.
export const devKeyboard = defineUiTree({
  name: KEYBOARD_TREE,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      // Q, as on the engine keyboard. While the address pad shows instead, Q
      // is hidden and focus falls back to the first stop, the pad's 1.
      initialFocus: "key_q",
      textEntryTarget: getGameState().ui.textEntry,
      accessibleName: "On-screen keyboard",
      role: "group",
    },
    panel({ wrap: false, localState: keyboardState.scope }, [
      Text({ content: "", fontSize: 22, color: COLOR_ACCENT, bind: kb.title.get() }),
      HStack({ gap: 0, align: "center", fill: FILL_WELL, padding: 10 }, [
        Text({ content: "", fontSize: 18, bind: getGameState().ui.textEntry, visibleWhen: kb.masked.is(false) }),
        Text({
          content: "",
          fontSize: 18,
          mask: true,
          bind: getGameState().ui.textEntry,
          visibleWhen: kb.masked.is(true),
        }),
        Text({ content: "_", fontSize: 18, color: COLOR_ACCENT }),
      ]),
      Text({ content: "", fontSize: 13, color: COLOR_MUTED, bind: kb.hint.get() }),
      // The row keeps the pad its own size rather than the panel's width.
      HStack({ gap: 0, align: "start", visibleWhen: kb.layout.is("address") }, [
        VStack({ width: 240, align: "stretch" }, [
          Grid(
            { gap: 8, align: "stretch", cols: 3, focus: { policy: "spatial" } },
            ADDRESS_PAD.map((ch) => charKey("pad", ch, 9)),
          ),
        ]),
      ]),
      Grid(
        { gap: 6, align: "stretch", cols: 10, focus: { policy: "spatial" }, visibleWhen: kb.layout.is("letters") },
        LETTER_ROWS.flatMap((row) => row.split("").map((ch) => charKey("key", ch))),
      ),
      HStack({ gap: 14, align: "center", focus: { policy: "linear" } }, [
        editKey("secondary", "key_backspace", "DEL", "kbBackspace", "text_backspace"),
        VStack({ visibleWhen: kb.layout.is("letters") }, [
          editKey("secondary", "key_space", "SPACE", "kbAppend_space", "text_space"),
        ]),
        Spacer({ flexGrow: 1 }),
        editKey("primary", "key_done", "DONE", COMMIT_TEXT_ENTRY_ACTION, "text_commit"),
      ]),
      footer(
        [action("nav", { id: "key_cancel", label: "CANCEL", onPress: CLOSE_DIALOG_ACTION }), Glyph({ command: "nav_cancel" })],
        [hint("Or type: Enter is done, Esc cancels")],
      ),
    ]),
  ),
});

/// An editable value as one row: its caption, its value in a dark well (or
/// `empty`, muted, while it is blank), and the EDIT box that opens the
/// keyboard for it. The box is named by the caption, so it reads as "Address".
function field(
  id: string,
  caption: string,
  edit: NamedReactionDescriptor,
  value: WidgetDescriptor,
  slot: typeof session.joinPassword,
  empty: string,
): WidgetDescriptor {
  return HStack({ gap: 10, align: "center" }, [
    VStack({ width: 84, align: "start" }, [Text({ id: `${id}Caption`, content: caption, fontSize: 14, color: COLOR_MUTED })]),
    HStack({ width: 220, padding: 8, gap: 0, align: "center", fill: FILL_WELL }, [
      value,
      Text({ content: empty, fontSize: 15, color: COLOR_MUTED, visibleWhen: stateEquals(slot, "") }),
    ]),
    action("secondary", { id, labelledBy: `${id}Caption`, valueText: [{ text: "EDIT" }], onPress: edit }),
  ]);
}

function passwordField(
  id: string,
  edit: NamedReactionDescriptor,
  slot: typeof session.joinPassword,
  empty: string,
): WidgetDescriptor {
  return field(id, "Password", edit, Text({ content: "", fontSize: 15, mask: true, bind: bindState(slot) }), slot, empty);
}

// Host and Join buttons. hostSession reads session.hostPassword; joinSession
// reads session.joinPassword and session.joinAddress. The two passwords are
// separate fields, so one typed to join never becomes the one a game hosts with.
// Hosting from the frontend lands in the lobby, in the Host menu's place, so
// the lobby always sits on the title and its CANCEL GAME returns there.
const host = defineReaction("frontend.hostSession", [hostSession(), closeDialog(), openMenu(LOBBY_MENU_NAME)]);
const join = defineReaction("frontend.joinSession", joinSession());
const stopHosting = defineReaction("frontend.stopHosting", endSession());
const leave = defineReaction("frontend.leaveSession", leaveSession());
const openHost = defineReaction("frontend.openHost", openMenu(HOST_MENU_NAME));
/** The title's LOBBY, while hosting. */
export const openLobby = defineReaction("frontend.openLobby", openMenu(LOBBY_MENU_NAME));
// The Host menu's OPEN LOBBY: the lobby takes the Host menu's place.
const showLobby = defineReaction("frontend.showLobby", [closeDialog(), openMenu(LOBBY_MENU_NAME)]);
/** The lobby's CANCEL GAME: end the session, back to the title. */
export const cancelLobby = defineReaction("frontend.lobby.cancel", [endSession(), closeDialog()]);
const openJoin = defineReaction("frontend.openJoin", openMenu(JOIN_MENU_NAME));
// From the title menu's notice straight to the field to fix: the keyboard
// commits into the Join menu beneath it, which then opens on JOIN.
const retypeAddress = defineReaction("frontend.retypeAddress", [
  openMenu(JOIN_MENU_NAME),
  ...editFieldSteps(session.joinAddress, JOIN_ADDRESS_FIELD),
]);
const retypePassword = defineReaction("frontend.retypePassword", [
  openMenu(JOIN_MENU_NAME),
  ...editFieldSteps(session.joinPassword, JOIN_PASSWORD_FIELD),
]);

export const isHosting = stateEquals(session.hosting, true);
export const notHosting = stateEquals(session.hosting, false);
const isOffline = stateEquals(session.connection, "offline");
export const isConnecting = stateEquals(session.connection, "connecting");
const isWaiting = stateEquals(session.connection, "waiting");

// With a `saves` block the engine refuses hosting and joining until a
// character is chosen, so the menus say so and offer the way there instead
// of a press that would only warn.
const { saves } = getGameState();
export const characterChosen = stateCompare(saves.character, "atLeast", 0);
export const noCharacter = stateEquals(saves.character, -1);

/// While no character is chosen: why the menu cannot go on, and the press
/// that fixes it.
function characterNotice(id: string, why: string): WidgetDescriptor {
  return callout(
    "notice",
    [
      headline("notice", "CHOOSE A CHARACTER FIRST"),
      hint(why),
      action("primary", { id, label: "CHARACTERS", onPress: openPlay }),
    ],
    noCharacter,
  );
}

/// The host's address, for players to type.
function hostAddressLine(): WidgetDescriptor {
  return Text({
    content: "",
    fontSize: 16,
    color: COLOR_ACCENT,
    bind: bindState(session.hostAddress, { format: "HOSTING ON {}" }),
  });
}

/// While hosting: where players dial, whether they need a password, and what
/// they wait for.
export function hostingStatus(): WidgetDescriptor {
  return callout(
    "status",
    [
      hostAddressLine(),
      body("Password protected: players need your password.", stateEquals(session.passwordProtected, true)),
      body("No password: anyone can join.", stateEquals(session.passwordProtected, false)),
      hint("Players wait here until you start a level."),
    ],
    isHosting,
  );
}

/// While admitted and held: why.
function waitReasons(): WidgetDescriptor[] {
  const reason = (value: string, notice: string, detail: string) =>
    callout("status", [headline("status", notice), hint(detail)], stateEquals(session.waitReason, value));
  return [
    reason("host", "CONNECTED", "Waiting for the host to start a level."),
    reason("modDiffers", "YOUR MOD DIFFERS FROM THE HOST'S", "Update it to match, then join again."),
    reason("levelDiffers", "YOU DON'T HAVE THE HOST'S LEVEL", "Get the host's map, or wait for the next level."),
  ];
}

/// While a dial is in flight.
function connectingStatus(): WidgetDescriptor {
  return callout(
    "status",
    [
      Text({
        content: "",
        fontSize: 16,
        color: COLOR_ACCENT,
        bind: bindState(session.joinAddress, { format: "CONNECTING TO {}..." }),
      }),
    ],
    isConnecting,
  );
}

const SESSION_MENU_WIDTH = 480;

function sessionMenu(name: string, heading: string, initialFocus: string, rows: WidgetDescriptor[]) {
  return defineUiTree({
    name,
    hideBelow: true,
    tree: Tree(
      {
        anchor: "center",
        offset: [0, 0],
        captureMode: "capture",
        initialFocus,
        accessibleName: heading,
        role: "group",
      },
      panel({ width: SESSION_MENU_WIDTH }, [title(heading), ...rows, footer(backAction(`${name}.back`))]),
    ),
  });
}

// Opens on START HOSTING: the password is optional, so one press hosts, and
// the lobby takes this menu's place. While hosting, OPEN LOBBY returns there.
export const hostMenu = sessionMenu(HOST_MENU_NAME, "HOST A GAME", "hostStart", [
  VStack({ gap: 14, align: "stretch", visibleWhen: isHosting }, [
    hostingStatus(),
    action("primary", { id: "hostLobby", label: "OPEN LOBBY", onPress: showLobby }),
    action("session", { id: "hostStop", label: "STOP HOSTING", onPress: stopHosting }),
  ]),
  VStack({ gap: 14, align: "stretch", visibleWhen: notHosting }, [
    endCauseNotice("hostFailed"),
    body("Finishing the last session...", isConnecting),
    characterNotice("hostCharacters", "Players join you as the character you play."),
    passwordField("hostPassword", editHostPassword, session.hostPassword, "None: anyone can join"),
    action("primary", { id: "hostStart", label: "START HOSTING", onPress: host, visibleWhen: characterChosen }),
  ]),
]);

// Opens on JOIN: the address is remembered from the last join, so a rejoin is
// one press. A blank address answers with its notice, one move from the field.
export const joinMenu = sessionMenu(JOIN_MENU_NAME, "JOIN A GAME", "joinStart", [
  VStack({ gap: 14, align: "stretch", visibleWhen: isConnecting }, [
    connectingStatus(),
    action("session", { id: "joinCancel", label: "STOP CONNECTING", onPress: leave }),
  ]),
  VStack({ gap: 14, align: "stretch", visibleWhen: isWaiting }, [
    ...waitReasons(),
    action("session", { id: "joinLeave", label: "LEAVE GAME", onPress: leave }),
  ]),
  VStack({ gap: 14, align: "stretch", visibleWhen: isOffline }, [
    ...endCauseNotices(JOIN_END_CAUSES),
    characterNotice("joinCharacters", "You join as the character you choose, with its gear."),
    VStack({ gap: 10, align: "stretch" }, [
      field(
        "joinAddress",
        "Address",
        editJoinAddress,
        Text({ content: "", fontSize: 15, bind: bindState(session.joinAddress) }),
        session.joinAddress,
        "Not set",
      ),
      passwordField("joinPassword", editPassword, session.joinPassword, "None"),
    ]),
    action("primary", { id: "joinStart", label: "JOIN", onPress: join, visibleWhen: characterChosen }),
  ]),
]);

// Per-key named reactions the on-screen keyboard's buttons reference
// (`onPress`): each appends its character to the writable engine slot
// `ui.textEntry`, and backspace pops one grapheme. They are mod-wide, so the
// keyboard types at the frontend and in every level. The keyboards name them,
// so editing a layout needs only matching names here. The `done` key is not
// here: its `onPress` is the reserved `ui.commitTextEntry` sentinel.
const { textEntry } = getGameState().ui;
const keyboardReactions: NamedReactionDescriptor[] = [
  ..."abcdefghijklmnopqrstuvwxyz0123456789"
    .split("")
    .map((ch) => defineReaction(`kbAppend_${ch}`, appendText(textEntry, ch))),
  defineReaction("kbAppend_space", appendText(textEntry, " ")),
  // Address punctuation: an IPv4 address and its port.
  defineReaction("kbAppend_dot", appendText(textEntry, ".")),
  defineReaction("kbAppend_colon", appendText(textEntry, ":")),
  defineReaction("kbAppend_dash", appendText(textEntry, "-")),
  defineReaction("kbAppend_underscore", appendText(textEntry, "_")),
  defineReaction("kbBackspace", backspaceText(textEntry)),
];

// --- Characters, runs and the lobby -------------------------------------------
// The engine owns the save slots and publishes them as `getGameState().saves`;
// these reactions press its verbs, one per slot row, under 0-based names that
// match `forEach`'s row index. `frontend-saves.ts` lays out the screens that
// press them. The engine refuses a verb that does not apply (an occupied
// slot for CREATE, another character's run for CONTINUE while hosting) with a
// warning, so the screens show each control only where it can work.

/** The dev mod's `saves` block: how many character and run slots it keeps. */
export const CHARACTER_SLOTS = 4;
export const RUN_SLOTS = 3;

/** New game starts a run on the catalog's campaign map. */
export const NEW_GAME_MAP = mapsTagged("campaign")[0].id;

type CharacterVerb = "select" | "name" | "create" | "askDelete" | "delete";
type RunVerb = "continue" | "new" | "askDelete" | "delete";

/** The reaction a character row's control presses for slot `i`. */
export function characterReaction(verb: CharacterVerb, i: number): string {
  return `frontend.characters.${verb}.${i}`;
}

/** The reaction a run row's control presses for slot `i`. */
export function runReaction(verb: RunVerb, i: number): string {
  return `frontend.runs.${verb}.${i}`;
}

/** The confirmation each DELETE opens: the safe choice is focused. */
export function deleteCharacterDialogName(i: number): string {
  return `frontend.characters.confirmDelete.${i}`;
}

export function deleteRunDialogName(i: number): string {
  return `frontend.runs.confirmDelete.${i}`;
}

const CHARACTER_NAME_FIELD: KeyboardField = {
  layout: "letters",
  masked: false,
  title: "NAME YOUR CHARACTER",
  hint: "Other players see this name.",
};

/** The 0-based row indices of `count` save slots. */
export function slotIndices(count: number): number[] {
  return Array.from({ length: count }, (_, i) => i);
}

// CREATE opens the keyboard on `ui.textEntry` itself, with no field target:
// a target would take the text out of `ui.textEntry` before the commit's
// `createCharacter` reads it as the name. So the entry is emptied first, and
// emptied again once the character has its name. A created character is
// selected, so the runs screen follows.
const characterReactions: NamedReactionDescriptor[] = slotIndices(CHARACTER_SLOTS).flatMap((i) => [
  defineReaction(characterReaction("select", i), [selectCharacter(i), openMenu(RUNS_MENU_NAME)]),
  defineReaction(characterReaction("name", i), [
    clearText(textEntry),
    kb.layout.set(CHARACTER_NAME_FIELD.layout),
    kb.masked.set(CHARACTER_NAME_FIELD.masked),
    kb.title.set(CHARACTER_NAME_FIELD.title),
    kb.hint.set(CHARACTER_NAME_FIELD.hint),
    openTextEntry(characterReaction("create", i)),
  ]),
  defineReaction(characterReaction("create", i), [
    createCharacter(i),
    clearText(textEntry),
    openMenu(RUNS_MENU_NAME),
  ]),
  defineReaction(characterReaction("askDelete", i), openMenu(deleteCharacterDialogName(i))),
  defineReaction(characterReaction("delete", i), [deleteCharacter(i), closeDialog()]),
]);

// CONTINUE and NEW GAME load a level, which closes the frontend; the lobby
// presses the same two, and the players waiting there follow.
const runReactions: NamedReactionDescriptor[] = slotIndices(RUN_SLOTS).flatMap((i) => [
  defineReaction(runReaction("continue", i), continueRun(i)),
  defineReaction(runReaction("new", i), newRun(i, NEW_GAME_MAP)),
  defineReaction(runReaction("askDelete", i), openMenu(deleteRunDialogName(i))),
  defineReaction(runReaction("delete", i), [deleteRun(i), closeDialog()]),
]);

const sessionReactions: NamedReactionDescriptor[] = [
  ...keyboardReactions,
  editPassword,
  editHostPassword,
  editJoinAddress,
  host,
  join,
  stopHosting,
  leave,
  openHost,
  openJoin,
  retypeAddress,
  retypePassword,
  openLobby,
  showLobby,
  cancelLobby,
  ...characterReactions,
  ...runReactions,
];

/// The title menu's answer to a notice: rejoin, or go straight to the field
/// that needs fixing. A wrong mod has nothing to retry.
const END_CAUSE_ACTIONS: Partial<Record<EndCause, { label: string; onPress: NamedReactionDescriptor }>> = {
  sessionEnded: { label: "JOIN AGAIN", onPress: openJoin },
  hostLost: { label: "JOIN AGAIN", onPress: openJoin },
  connectFailed: { label: "TRY AGAIN", onPress: openJoin },
  refusedPassword: { label: "ENTER PASSWORD", onPress: retypePassword },
  badAddress: { label: "ENTER ADDRESS", onPress: retypeAddress },
  hostFailed: { label: "TRY AGAIN", onPress: openHost },
};

/// Each notice, with its answer as the title's primary action.
function titleNotices(): WidgetDescriptor[] {
  return ALL_END_CAUSES.map((cause) => {
    const answer = END_CAUSE_ACTIONS[cause];
    return endCauseNotice(
      cause,
      answer === undefined
        ? undefined
        : action("primary", { id: `titleNotice.${cause}`, label: answer.label, onPress: answer.onPress }),
    );
  });
}

/** Opens the tabbed options screen; the pause menu opens it too. */
export const openOptions = defineReaction("frontend.openOptions", openMenu(OPTIONS_MENU_NAME));

// The title menu leads with the session: where this machine hosts, what a
// join is doing, or why the last one ended, with the one press that answers
// it. Its initial focus names a notice button: with none showing, focus falls
// back to the first visible stop, PLAY for a player offline. The title is the
// root, so nothing is beneath it to go back to: its footer's way out is EXIT.
// PLAY leads to characters, then runs; while hosting, LOBBY takes its place.
// While a session runs, HOST or JOIN opens that session's status and the
// action that ends it, so it reads VIEW SESSION. LEVEL SELECT is the dev
// path straight into a catalog map, outside any run.
export const frontendMenu = defineUiTree({
  name: TITLE_MENU_NAME,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: "titleNotice.connectFailed",
      accessibleName: "PostRetro title menu",
      role: "group",
    },
    panel({ width: 400 }, [
      title("POSTRETRO", 36),
      hostingStatus(),
      callout("status", [headline("status", "CONNECTING...")], isConnecting),
      ...waitReasons(),
      // A client lands here when its session ends: say why.
      ...titleNotices(),
      VStack({ gap: 8, align: "stretch" }, [
        // A connected client plays the host's levels, and hosts nothing.
        VStack({ gap: 8, align: "stretch", visibleWhen: isOffline }, [
          action("secondary", { id: "frontendPlay", label: "PLAY", onPress: openPlay, visibleWhen: notHosting }),
          action("secondary", { id: "frontendLobby", label: "LOBBY", onPress: openLobby, visibleWhen: isHosting }),
          action("secondary", {
            id: "frontendHost",
            label: "HOST",
            onPress: openHost,
            valueText: [{ when: [isHosting], text: "VIEW SESSION" }],
          }),
        ]),
        // A host joins nothing.
        action("secondary", {
          id: "frontendJoin",
          label: "JOIN",
          onPress: openJoin,
          visibleWhen: notHosting,
          valueText: [
            { when: [isConnecting], text: "VIEW SESSION" },
            { when: [isWaiting], text: "VIEW SESSION" },
          ],
        }),
        // A connected client plays the host's levels, so it picks none.
        action("secondary", {
          id: "frontendLevels",
          label: "LEVEL SELECT",
          onPress: openLevelSelect,
          visibleWhen: isOffline,
        }),
        action("secondary", { id: "frontendOptions", label: "OPTIONS", onPress: openOptions }),
      ]),
      footer([action("nav", { id: "frontendExit", label: "EXIT", onPress: askExit })]),
    ]),
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

/// Fluid reflection tiers, cheapest first: off, the probe term alone (low),
/// then the screen-space layer by each tier's preset. A cost choice only: the
/// reflection style is the game's, never the player's.
const REFLECTION_QUALITY_CHOICES = [
  { value: "off", id: "optionsReflectionOff", label: "OFF" },
  { value: "low", id: "optionsReflectionLow", label: "LOW" },
  { value: "medium", id: "optionsReflectionMedium", label: "MEDIUM" },
  { value: "high", id: "optionsReflectionHigh", label: "HIGH" },
  { value: "very_high", id: "optionsReflectionVeryHigh", label: "VERY HIGH" },
  { value: "ultra", id: "optionsReflectionUltra", label: "ULTRA" },
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
  ...REFLECTION_QUALITY_CHOICES.map(({ value }) =>
    defineReaction(
      `frontend.options.reflectionQuality.${value}`,
      updateState(options.reflectionQuality, value),
    ),
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
      entries: [{ upTo: 0, color: COLOR_UNSELECTED }, { color: COLOR_ACCENT }],
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
      entries: [{ upTo: 0, color: COLOR_UNSELECTED }, { color: COLOR_ACCENT }],
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
    // Opens the engine controls panel: an EDIT box, as a field's is.
    optionValue(
      action("secondary", {
        id: "optionsRebind",
        labelledBy: "optionsRebindLabel",
        valueText: [{ text: "EDIT" }],
        onPress: OPEN_CONTROLS_ACTION,
      }),
    ),
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
    optionLabel("optionsReflectionQualityLabel", "REFLECTIONS"),
    optionChoices(
      REFLECTION_QUALITY_CHOICES.map(({ value, id, label }) =>
        radioChoice(
          id,
          label,
          stateEquals(options.reflectionQuality, value),
          `frontend.options.reflectionQuality.${value}`,
        ),
      ),
    ),
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
          { gap: 20, align: "stretch", role: "tablist", focus: { policy: "linear", wrap: true } },
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
        footer(backAction("optionsBack")),
      ],
    ),
  ),
});

export const frontendReactions: NamedReactionDescriptor[] = [
  openPlay,
  openLevelSelect,
  ...sessionReactions,
  openOptions,
  askExit,
  askQuit,
  ...frontendStartReactions,
  ...optionReactions,
  ...optionsTabReactions,
];
