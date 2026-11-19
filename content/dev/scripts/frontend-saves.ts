// The dev frontend's save screens: characters, runs, the host lobby, and the
// confirmation each DELETE opens. They follow the visual grammar at the top of
// `frontend-menu.ts`, with its helpers. Every reaction they press lives there,
// in `frontendReactions`, so this module only lays the screens out.
//
// The flow: the title's PLAY opens characters; SELECT, or CREATE once the
// name is typed, opens runs, where CONTINUE and NEW GAME load the level.
// Hosting lands in the lobby, which lists the players joining and starts the
// host's run for everyone.
// See: docs/scripting-reference.md (Characters, runs and checkpoints)

import {
  HStack,
  Text,
  Tree,
  VStack,
  bindState,
  defineUiTree,
  forEach,
  getGameState,
  stateEquals,
  type Predicate,
  type WidgetDescriptor,
} from "postretro/ui";
import {
  CHARACTER_SLOTS,
  CHARACTERS_MENU_NAME,
  COLOR_ACCENT,
  COLOR_BODY,
  COLOR_MUTED,
  COLOR_NOTICE,
  LOBBY_MENU_NAME,
  RUN_SLOTS,
  RUNS_MENU_NAME,
  action,
  backAction,
  body,
  callout,
  cancelLobby,
  characterReaction,
  confirmDialog,
  deleteCharacterDialogName,
  deleteRunDialogName,
  endCauseNotice,
  footer,
  headline,
  hint,
  hostingStatus,
  isConnecting,
  isHosting,
  noCharacter,
  notHosting,
  openLevelSelect,
  panel,
  runReaction,
  slotIndices,
  title,
} from "./frontend-menu";

const { saves, session } = getGameState();

type CharacterRow = (typeof saves.characters)[number];
type RunRow = (typeof saves.runs)[number];
type PlayerRow = (typeof session.players)[number];

const SAVES_MENU_WIDTH = 600;
/// The information column of a row: fixed, so each row's controls line up in
/// columns and Up and Down move straight between rows.
const ROW_INFO_WIDTH = 250;

/// `children`, shown only while `when` holds: a second condition on a node
/// that already has one.
function only(when: Predicate, children: WidgetDescriptor[]): WidgetDescriptor {
  return VStack({ gap: 0, align: "stretch", visibleWhen: when }, children);
}

/// A section caption: information, sentence case, muted.
function caption(content: string): WidgetDescriptor {
  return Text({ content, fontSize: 14, color: COLOR_MUTED });
}

/// The rows as one spatial focus group: Up and Down cross rows, Left and
/// Right cross a row's controls, and leaving the last row reaches the footer.
function rows(children: WidgetDescriptor[]): WidgetDescriptor {
  return VStack({ gap: 10, align: "stretch", focus: { policy: "spatial" } }, children);
}

/// An occupied slot whose file this build cannot read: only DELETE applies.
/// Empty slots read unreadable too, so the note also asks for occupied.
function unreadableNote(occupied: CharacterRow["occupied"], readable: CharacterRow["readable"]): WidgetDescriptor {
  return only(stateEquals(occupied, true), [
    Text({
      content: "This build cannot read this save",
      fontSize: 13,
      color: COLOR_NOTICE,
      visibleWhen: stateEquals(readable, false),
    }),
  ]);
}

// --- Characters ----------------------------------------------------------------

/// A character slot: its name, or why it has none, and the controls that
/// apply to it. An unreadable save can only be deleted.
function characterRow(row: CharacterRow, i: number): WidgetDescriptor {
  const occupied = stateEquals(row.occupied, true);
  const readable = stateEquals(row.readable, true);
  return HStack({ gap: 10, align: "center" }, [
    VStack({ width: ROW_INFO_WIDTH, gap: 2, align: "start" }, [
      Text({ content: "", fontSize: 16, color: COLOR_BODY, bind: bindState(row.name), visibleWhen: readable }),
      Text({ content: "Selected", fontSize: 13, color: COLOR_ACCENT, visibleWhen: stateEquals(saves.character, i) }),
      unreadableNote(row.occupied, row.readable),
      Text({ content: "Empty slot", fontSize: 14, color: COLOR_MUTED, visibleWhen: stateEquals(row.occupied, false) }),
    ]),
    action("secondary", {
      id: "characterSelect",
      label: "SELECT",
      onPress: characterReaction("select", i),
      visibleWhen: readable,
    }),
    action("secondary", {
      id: "characterCreate",
      label: "CREATE",
      onPress: characterReaction("name", i),
      visibleWhen: stateEquals(row.occupied, false),
    }),
    action("secondary", {
      id: "characterDelete",
      label: "DELETE",
      onPress: characterReaction("askDelete", i),
      visibleWhen: occupied,
    }),
  ]);
}

export const charactersMenu = defineUiTree({
  name: CHARACTERS_MENU_NAME,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      // The first slot's SELECT, or its CREATE while it is empty.
      initialFocus: "characterSelect.0",
      accessibleName: "Characters",
      role: "group",
    },
    panel({ width: SAVES_MENU_WIDTH }, [
      title("CHARACTERS"),
      body("Choose who you play as. Gear and progress stay with the character."),
      rows(forEach(saves.characters, characterRow)),
      footer(backAction(`${CHARACTERS_MENU_NAME}.back`)),
    ]),
  ),
});

// --- Runs ----------------------------------------------------------------------

/// What a run slot holds: its level, whose run it is and when it saved, or why
/// it holds none.
function runInfo(row: RunRow): WidgetDescriptor {
  const readable = stateEquals(row.readable, true);
  return VStack({ width: ROW_INFO_WIDTH, gap: 2, align: "start" }, [
    Text({ content: "", fontSize: 16, color: COLOR_BODY, bind: bindState(row.levelName), visibleWhen: readable }),
    only(readable, [
      HStack({ gap: 8, align: "center" }, [
        Text({ content: "", fontSize: 13, color: COLOR_MUTED, bind: bindState(row.characterName) }),
        Text({ content: "", fontSize: 13, color: COLOR_MUTED, bind: bindState(row.savedAt) }),
      ]),
    ]),
    unreadableNote(row.occupied, row.readable),
    Text({ content: "Empty slot", fontSize: 14, color: COLOR_MUTED, visibleWhen: stateEquals(row.occupied, false) }),
  ]);
}

/// CONTINUE for a run this build can read, and NEW GAME for any slot. `prefix`
/// keeps the runs screen's ids apart from the lobby's.
function runStarts(row: RunRow, i: number, prefix: string): WidgetDescriptor[] {
  return [
    action("secondary", {
      id: `${prefix}Continue`,
      label: "CONTINUE",
      onPress: runReaction("continue", i),
      visibleWhen: stateEquals(row.readable, true),
    }),
    action("secondary", { id: `${prefix}New`, label: "NEW GAME", onPress: runReaction("new", i) }),
  ];
}

function runRow(row: RunRow, i: number): WidgetDescriptor {
  return HStack({ gap: 10, align: "center" }, [
    runInfo(row),
    ...runStarts(row, i, "run"),
    action("secondary", {
      id: "runDelete",
      label: "DELETE",
      onPress: runReaction("askDelete", i),
      visibleWhen: stateEquals(row.occupied, true),
    }),
  ]);
}

/// Who the runs are played as: the selected character's name.
function playingAs(): WidgetDescriptor[] {
  return forEach(saves.characters, (row, i) =>
    Text({
      content: "",
      fontSize: 16,
      color: COLOR_ACCENT,
      bind: bindState(row.name, { format: "Playing as {}" }),
      visibleWhen: stateEquals(saves.character, i),
    }),
  );
}

export const runsMenu = defineUiTree({
  name: RUNS_MENU_NAME,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      // The first slot's CONTINUE, or its NEW GAME while it is empty.
      initialFocus: "runContinue.0",
      accessibleName: "Runs",
      role: "group",
    },
    panel({ width: SAVES_MENU_WIDTH }, [
      title("RUNS"),
      ...playingAs(),
      callout(
        "notice",
        [headline("notice", "NO CHARACTER CHOSEN"), hint("Go back and choose one to start or continue a run.")],
        noCharacter,
      ),
      hint("Continue from the last level start or checkpoint. New game keeps your character's gear."),
      rows(forEach(saves.runs, runRow)),
      footer(backAction(`${RUNS_MENU_NAME}.back`)),
    ]),
  ),
});

// --- Host lobby ----------------------------------------------------------------

const PLAYER_STATUS: ReadonlyArray<{ status: "loading" | "inPlay" | "down" | "reconnecting"; text: string }> = [
  { status: "loading", text: "Joining" },
  { status: "reconnecting", text: "Reconnecting" },
  { status: "inPlay", text: "In game" },
  { status: "down", text: "Down" },
];

/// One player at the lobby: their name and how their join is going.
function playerRow(row: PlayerRow): WidgetDescriptor {
  return HStack({ gap: 10, align: "center" }, [
    VStack({ width: ROW_INFO_WIDTH, align: "start" }, [
      Text({ content: "", fontSize: 15, color: COLOR_BODY, bind: bindState(row.name) }),
    ]),
    ...PLAYER_STATUS.map(({ status, text }) =>
      Text({
        content: text,
        fontSize: 13,
        color: status === "inPlay" ? COLOR_ACCENT : COLOR_NOTICE,
        visibleWhen: stateEquals(row.status, status),
      }),
    ),
  ]);
}

function lobbyRunRow(row: RunRow, i: number): WidgetDescriptor {
  return HStack({ gap: 10, align: "center" }, [runInfo(row), ...runStarts(row, i, "lobby")]);
}

// The lobby takes the Host menu's place on the title, so BACK leaves the game
// hosted and the title shows it; CANCEL GAME ends it and returns there too.
// The host starts one of its own runs and the players here follow it in.
export const lobbyMenu = defineUiTree({
  name: LOBBY_MENU_NAME,
  hideBelow: true,
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: "lobbyContinue.0",
      accessibleName: "Lobby",
      role: "group",
    },
    panel({ width: SAVES_MENU_WIDTH }, [
      title("LOBBY"),
      VStack({ gap: 14, align: "stretch", visibleWhen: isHosting }, [
        hostingStatus(),
        caption("Players"),
        VStack({ gap: 4, align: "stretch" }, forEach(session.players, playerRow)),
        hint("Players who join show here, and follow you into the run you start."),
        caption("Your runs"),
        rows(forEach(saves.runs, lobbyRunRow)),
        action("secondary", { id: "lobbyLevels", label: "LEVEL SELECT", onPress: openLevelSelect }),
        action("session", { id: "lobbyCancel", label: "CANCEL GAME", onPress: cancelLobby }),
      ]),
      VStack({ gap: 14, align: "stretch", visibleWhen: notHosting }, [
        endCauseNotice("hostFailed"),
        body("Finishing the last session...", isConnecting),
        body("You are not hosting. Go back to host a game."),
      ]),
      footer(backAction(`${LOBBY_MENU_NAME}.back`)),
    ]),
  ),
});

// --- Delete confirmations ------------------------------------------------------

/// One confirmation per slot: it names what it deletes, and its focus opens on
/// CANCEL, so a repeated press never deletes.
export const deleteDialogs = [
  ...slotIndices(CHARACTER_SLOTS).map((i) =>
    confirmDialog(
      deleteCharacterDialogName(i),
      "DELETE THIS CHARACTER?",
      "DELETE",
      characterReaction("delete", i),
      `deleteCharacter${i}`,
      [
        Text({ content: "", fontSize: 16, color: COLOR_BODY, bind: bindState(saves.characters[i].name) }),
        hint("Its runs stay, but only to delete."),
      ],
    ),
  ),
  ...slotIndices(RUN_SLOTS).map((i) =>
    confirmDialog(deleteRunDialogName(i), "DELETE THIS RUN?", "DELETE", runReaction("delete", i), `deleteRun${i}`, [
      Text({ content: "", fontSize: 16, color: COLOR_BODY, bind: bindState(saves.runs[i].levelName) }),
      Text({ content: "", fontSize: 13, color: COLOR_MUTED, bind: bindState(saves.runs[i].savedAt) }),
    ]),
  ),
];

/** Every save screen, for the manifest's `uiTrees`. */
export const saveScreens = [charactersMenu, runsMenu, lobbyMenu, ...deleteDialogs];
