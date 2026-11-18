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
  Switch,
  Text,
  Tree,
  VStack,
  accessibilityAction,
  bindState,
  displayModeAction,
  defineUiTree,
  endSession,
  getGameState,
  hostSession,
  joinSession,
  KEYBOARD_TREE,
  leaveSession,
  loadLevel,
  openMenu,
  openTextEntry,
  stateEquals,
  ui,
  updateState,
  type Predicate,
  type ValueTextCase,
  type WidgetDescriptor,
} from "postretro/ui";

// Tests bundle this module standalone (`menu_tests`, `window_mode_sdk_tests`),
// so it takes no relative imports; `loading-screens.ts` imports from here.

/// The loading tree `loading-screens.ts` registers for catalog map `mapId`.
export function loadingTreeName(mapId: string): string {
  return `dev.loading.${mapId}`;
}

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

// --- Host and Join ---------------------------------------------------------
// The engine owns entry, admission and the session facts; these menus only
// show them. Each end cause, connection phase and wait reason is data the
// engine publishes; the wording is the mod's.
//
// Each menu shows one state at a time, keyed on the session facts: the form
// while offline, and only the status and its one way out while connecting,
// waiting or hosting. Mutually exclusive groups share a place on screen, so
// the status group sits first in tree order: when the focused button hides,
// focus falls back to the first visible stop, the status group's own button.

const HOST_MENU_NAME = "frontend.host";
const JOIN_MENU_NAME = "frontend.join";
const { session } = getGameState();

const COLOR_NOTICE: [number, number, number, number] = [0.98, 0.68, 0.22, 1.0];

/// The notice for each `session.endCause` value: why the last session or
/// attempt ended. ASCII only: it renders in every bundled typeface.
export const END_CAUSE_NOTICES = {
  sessionEnded: "THE HOST ENDED THE SESSION",
  hostLost: "LOST CONNECTION TO THE HOST",
  connectFailed: "COULD NOT REACH THE HOST",
  refusedMod: "THE HOST IS PLAYING A DIFFERENT MOD",
  refusedPassword: "PASSWORD REFUSED",
  badAddress: "ENTER THE HOST'S ADDRESS",
  hostFailed: "COULD NOT START HOSTING",
} as const;

type EndCause = keyof typeof END_CAUSE_NOTICES;

/// What to do about a notice, where there is something to do.
const END_CAUSE_HINTS: Partial<Record<EndCause, string>> = {
  connectFailed: "CHECK THE ADDRESS, AND THAT THE HOST RUNS YOUR BUILD",
  refusedMod: "BOTH PLAYERS NEED THE SAME MOD",
  refusedPassword: "ENTER THE HOST'S PASSWORD, THEN JOIN AGAIN",
  badAddress: "IPV4 DIGITS, LIKE 192.168.1.20. NAMES DO NOT WORK",
  hostFailed: "ANOTHER PROGRAM MAY BE USING THE PORT",
};

/// A cause's headline and, if it has one, its hint.
function endCauseNotice(cause: EndCause, action?: WidgetDescriptor): WidgetDescriptor {
  const hint = END_CAUSE_HINTS[cause];
  return VStack({ gap: 4, align: "stretch", visibleWhen: stateEquals(session.endCause, cause) }, [
    Text({ content: END_CAUSE_NOTICES[cause], fontSize: 16, color: COLOR_NOTICE }),
    ...(hint === undefined ? [] : [Text({ content: hint, fontSize: 13, color: COLOR_MUTED })]),
    ...(action === undefined ? [] : [action]),
  ]);
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
function editFieldSteps(target: typeof session.password, field: KeyboardField) {
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
  hint: "IPV4, LIKE 192.168.1.20. ADD :PORT ONLY IF THE HOST SET ONE",
};
const JOIN_PASSWORD_FIELD: KeyboardField = {
  layout: "letters",
  masked: true,
  title: "PASSWORD",
  hint: "THE HOST'S PASSWORD, EXACTLY. EMPTY FOR NONE",
};
const HOST_PASSWORD_FIELD: KeyboardField = {
  layout: "letters",
  masked: true,
  title: "PASSWORD FOR YOUR GAME",
  hint: "PLAYERS TYPE THIS TO JOIN. EMPTY FOR AN OPEN GAME",
};

const editJoinAddress = defineReaction(
  "frontend.editJoinAddress",
  editFieldSteps(session.joinAddress, JOIN_ADDRESS_FIELD),
);
const editPassword = defineReaction(
  "frontend.editPassword",
  editFieldSteps(session.password, JOIN_PASSWORD_FIELD),
);
const editHostPassword = defineReaction(
  "frontend.editHostPassword",
  editFieldSteps(session.password, HOST_PASSWORD_FIELD),
);

const COMMIT_TEXT_ENTRY_ACTION = "ui.commitTextEntry";

/// The key reaction name for one character.
function keyReaction(ch: string): string {
  const named: Record<string, string> = { ".": "dot", ":": "colon", "-": "dash", _: "underscore" };
  return `kbAppend_${named[ch] ?? ch}`;
}

function charKey(idPrefix: string, ch: string): WidgetDescriptor {
  const name = keyReaction(ch).slice("kbAppend_".length);
  return Button({ id: `${idPrefix}_${name}`, label: ch.toUpperCase(), onPress: keyReaction(ch) });
}

/// Digits and address punctuation, phone-pad order: an address is typed in a
/// few moves from the pad's first key.
const ADDRESS_PAD = ["1", "2", "3", "4", "5", "6", "7", "8", "9", ".", "0", ":"];
const LETTER_ROWS = ["1234567890", "qwertyuiop", "asdfghjkl.", "zxcvbnm:-_"];

/// An action key with the pad shortcut that presses it beside it. The engine
/// shortcuts find these keys by id (`key_space`, `key_backspace`, `key_done`).
function actionKey(id: string, label: string, onPress: string, command: CommandId): WidgetDescriptor {
  return HStack({ gap: 4, align: "center" }, [
    Button({ id, label, onPress }),
    Glyph({ command }),
  ]);
}

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
    VStack(
      {
        localState: keyboardState.scope,
        gap: 12,
        padding: 20,
        align: "stretch",
        fill: COLOR_PANEL,
        focus: { policy: "linear" },
      },
      [
        Text({ content: "", fontSize: 20, color: COLOR_ACCENT, bind: kb.title.get() }),
        HStack({ gap: 0, align: "center", fill: COLOR_INACTIVE, padding: 8 }, [
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
          Grid(
            { gap: 10, align: "stretch", cols: 3, focus: { policy: "spatial" } },
            ADDRESS_PAD.map((ch) => charKey("pad", ch)),
          ),
        ]),
        Grid(
          { gap: 6, align: "stretch", cols: 10, focus: { policy: "spatial" }, visibleWhen: kb.layout.is("letters") },
          LETTER_ROWS.flatMap((row) => row.split("").map((ch) => charKey("key", ch))),
        ),
        HStack({ gap: 12, align: "center", focus: { policy: "linear" } }, [
          VStack({ visibleWhen: kb.layout.is("letters") }, [
            actionKey("key_space", "SPACE", "kbAppend_space", "text_space"),
          ]),
          actionKey("key_backspace", "DEL", "kbBackspace", "text_backspace"),
          actionKey("key_done", "DONE", COMMIT_TEXT_ENTRY_ACTION, "text_commit"),
          actionKey("key_cancel", "CANCEL", CLOSE_DIALOG_ACTION, "nav_cancel"),
        ]),
        Text({ content: "OR TYPE ON A KEYBOARD: ENTER IS DONE, ESC CANCELS", fontSize: 12, color: COLOR_MUTED }),
      ],
    ),
  ),
});

/// A labelled field row: the button opens the keyboard, the value beside it
/// shows the field, or `empty` while it is blank.
function textField(
  id: string,
  label: string,
  edit: NamedReactionDescriptor,
  value: WidgetDescriptor,
  slot: typeof session.password,
  empty: string,
): WidgetDescriptor[] {
  return [
    Button({ id, label, onPress: edit }),
    HStack({ gap: 0, align: "center" }, [
      value,
      Text({ content: empty, fontSize: 14, color: COLOR_MUTED, visibleWhen: stateEquals(slot, "") }),
    ]),
  ];
}

function passwordField(id: string, edit: NamedReactionDescriptor, empty: string): WidgetDescriptor[] {
  return textField(
    id,
    "PASSWORD",
    edit,
    Text({ content: "", fontSize: 14, mask: true, bind: bindState(session.password) }),
    session.password,
    empty,
  );
}

/// Field rows in two aligned columns. A grid with no focus policy opens no
/// group: its buttons stay stops in the menu's own column.
function fieldGrid(rows: WidgetDescriptor[]): WidgetDescriptor {
  return Grid({ gap: 10, align: "center", cols: 2 }, rows);
}

// Host and Join buttons. Each reads session.password; joinSession also reads
// session.joinAddress.
const host = defineReaction("frontend.hostSession", hostSession());
const join = defineReaction("frontend.joinSession", joinSession());
const stopHosting = defineReaction("frontend.stopHosting", endSession());
const leave = defineReaction("frontend.leaveSession", leaveSession());
const openHost = defineReaction("frontend.openHost", openMenu(HOST_MENU_NAME));
const openJoin = defineReaction("frontend.openJoin", openMenu(JOIN_MENU_NAME));
// From the title menu's notice straight to the field to fix: the keyboard
// commits into the Join menu beneath it, which then opens on JOIN.
const retypeAddress = defineReaction("frontend.retypeAddress", [
  openMenu(JOIN_MENU_NAME),
  ...editFieldSteps(session.joinAddress, JOIN_ADDRESS_FIELD),
]);
const retypePassword = defineReaction("frontend.retypePassword", [
  openMenu(JOIN_MENU_NAME),
  ...editFieldSteps(session.password, JOIN_PASSWORD_FIELD),
]);

const isHosting = stateEquals(session.hosting, true);
const notHosting = stateEquals(session.hosting, false);
const isOffline = stateEquals(session.connection, "offline");
const isConnecting = stateEquals(session.connection, "connecting");
const isWaiting = stateEquals(session.connection, "waiting");

/// The host's address, for players to type.
function hostAddressLine(fontSize: number): WidgetDescriptor {
  return Text({
    content: "",
    fontSize,
    color: COLOR_ACCENT,
    bind: bindState(session.hostAddress, { format: "HOSTING ON {}" }),
  });
}

/// While admitted and held: why.
function waitReasons(): WidgetDescriptor[] {
  const reason = (value: string, notice: string, hint: string) =>
    VStack({ gap: 4, align: "stretch", visibleWhen: stateEquals(session.waitReason, value) }, [
      Text({ content: notice, fontSize: 16, color: COLOR_ACCENT }),
      Text({ content: hint, fontSize: 13, color: COLOR_MUTED }),
    ]);
  return [
    reason("host", "CONNECTED", "WAITING FOR THE HOST TO START A LEVEL"),
    reason("modDiffers", "YOUR MOD DIFFERS FROM THE HOST'S", "UPDATE IT TO MATCH, THEN JOIN AGAIN"),
    reason("levelDiffers", "YOU DON'T HAVE THE HOST'S LEVEL", "GET THE HOST'S MAP, OR WAIT FOR THE NEXT LEVEL"),
  ];
}

function sessionMenu(name: string, title: string, initialFocus: string, rows: WidgetDescriptor[]) {
  return defineUiTree({
    name,
    hideBelow: true,
    tree: Tree(
      {
        anchor: "center",
        offset: [0, 0],
        captureMode: "capture",
        initialFocus,
        accessibleName: title,
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
          Text({ content: title, fontSize: 24, color: COLOR_ACCENT }),
          ...rows,
          Button({ id: `${name}.back`, label: "BACK", onPress: CLOSE_DIALOG_ACTION }),
          promptRow(),
        ],
      ),
    ),
  });
}

// Opens on START HOSTING: the password is optional, so one press hosts.
export const hostMenu = sessionMenu(HOST_MENU_NAME, "HOST A GAME", "hostStart", [
  VStack({ gap: 10, align: "stretch", visibleWhen: isHosting }, [
    hostAddressLine(18),
    Text({ content: "PLAYERS JOIN WITH THIS ADDRESS AND WAIT", fontSize: 13, color: COLOR_MUTED }),
    Text({ content: "HERE UNTIL YOU START A LEVEL", fontSize: 13, color: COLOR_MUTED }),
    Button({ id: "hostPlay", label: "START A LEVEL", onPress: openPlay }),
    Button({ id: "hostStop", label: "STOP HOSTING", onPress: stopHosting }),
  ]),
  VStack({ gap: 10, align: "stretch", visibleWhen: notHosting }, [
    endCauseNotice("hostFailed"),
    Text({
      content: "FINISHING THE LAST SESSION...",
      fontSize: 14,
      color: COLOR_MUTED,
      visibleWhen: isConnecting,
    }),
    fieldGrid(passwordField("hostPassword", editHostPassword, "NONE: ANYONE CAN JOIN")),
    Button({ id: "hostStart", label: "START HOSTING", onPress: host }),
  ]),
]);

// Opens on JOIN: the address is remembered from the last join, so a rejoin is
// one press. A blank address answers with its notice, one move from the field.
export const joinMenu = sessionMenu(JOIN_MENU_NAME, "JOIN A GAME", "joinStart", [
  VStack({ gap: 10, align: "stretch", visibleWhen: isConnecting }, [
    Text({
      content: "",
      fontSize: 16,
      color: COLOR_ACCENT,
      bind: bindState(session.joinAddress, { format: "CONNECTING TO {}..." }),
    }),
    Button({ id: "joinCancel", label: "CANCEL", onPress: leave }),
  ]),
  VStack({ gap: 10, align: "stretch", visibleWhen: isWaiting }, [
    ...waitReasons(),
    Button({ id: "joinLeave", label: "LEAVE", onPress: leave }),
  ]),
  VStack({ gap: 10, align: "stretch", visibleWhen: isOffline }, [
    ...endCauseNotices(JOIN_END_CAUSES),
    fieldGrid([
      ...textField(
        "joinAddress",
        "ADDRESS",
        editJoinAddress,
        Text({ content: "", fontSize: 14, bind: bindState(session.joinAddress) }),
        session.joinAddress,
        "NOT SET",
      ),
      ...passwordField("joinPassword", editPassword, "NONE"),
    ]),
    Button({ id: "joinStart", label: "JOIN", onPress: join }),
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

function titleNotices(): WidgetDescriptor[] {
  return ALL_END_CAUSES.map((cause) => {
    const action = END_CAUSE_ACTIONS[cause];
    return endCauseNotice(
      cause,
      action === undefined
        ? undefined
        : Button({ id: `titleNotice.${cause}`, label: action.label, onPress: action.onPress }),
    );
  });
}

/** Opens the tabbed options screen; the pause menu opens it too. */
export const openOptions = defineReaction("frontend.openOptions", openMenu(OPTIONS_MENU_NAME));

// The title menu leads with the session: where this machine hosts, what a
// join is doing, or why the last one ended, with the one press that answers
// it. Its initial focus names a notice button: with none showing, focus falls
// back to the first visible stop, PLAY for a player offline.
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
        VStack({ gap: 4, align: "stretch", visibleWhen: isHosting }, [
          hostAddressLine(14),
          Text({ content: "PLAYERS WAIT HERE UNTIL YOU START A LEVEL", fontSize: 13, color: COLOR_MUTED }),
        ]),
        Text({
          content: "CONNECTING...",
          fontSize: 14,
          color: COLOR_ACCENT,
          visibleWhen: isConnecting,
        }),
        VStack({ gap: 10, align: "stretch", visibleWhen: isWaiting }, waitReasons()),
        // A client lands here when its session ends: say why.
        ...titleNotices(),
        // A connected client plays the host's levels, and hosts nothing.
        VStack({ gap: 12, align: "stretch", visibleWhen: isOffline }, [
          Button({
            id: "frontendPlay",
            label: "PLAY",
            onPress: openPlay,
            valueText: [{ when: [isHosting], text: "START A LEVEL" }],
          }),
          Button({
            id: "frontendHost",
            label: "HOST",
            onPress: openHost,
            valueText: [{ when: [isHosting], text: "HOSTING" }],
          }),
        ]),
        // A host joins nothing.
        VStack({ gap: 12, align: "stretch", visibleWhen: notHosting }, [
          Button({
            id: "frontendJoin",
            label: "JOIN",
            onPress: openJoin,
            valueText: [
              { when: [isConnecting], text: "JOINING" },
              { when: [isWaiting], text: "JOINED" },
            ],
          }),
        ]),
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
  ...sessionReactions,
  openOptions,
  askExit,
  askQuit,
  ...frontendStartReactions,
  ...optionReactions,
  ...optionsTabReactions,
];
