// DEMO CONTENT — M13 G2 reactive-UI tabs demo (campaign-test.map).
//
// The headline G2 acceptance: a `localState` cell + a `role:"tablist"` strip
// where each tab `Button` carries a `Predicate` `bind` (drives the styleRanges
// highlight), the SAME predicate as `selected` (a11y state on the FocusRect),
// and an `onPress` that sets the cell; a `Switch(cell, map)` swaps the content
// panel. Highlight (visual) and `selected` (a11y) are wired from ONE predicate,
// so they agree by construction (the G2 invariant — see context/lib/ui.md §4).
//
// Authored as an `alwaysOn` HUD-layer tree registered through the level
// manifest's `uiTrees` (level-scoped). `cellWrite` onPress reactions are named
// here and referenced by the tab buttons (a Button `onPress` is a NAMED
// reaction — an inline descriptor is not accepted). Built entirely from G2-T4
// SDK factories; the scripting VM drops after registration, the engine owns the
// live UI every frame.
//
// See: docs/scripting-reference.md "Reactive UI", M13 G2 Task 5.

import {
  type NamedReactionDescriptor,
  defineReaction,
} from "postretro";
import {
  Button,
  HStack,
  Text,
  Tree,
  VStack,
  Switch,
  defineUiTree,
  ui,
  type UiTreeRegistration,
} from "postretro/ui";

const COLOR_OK: [number, number, number, number] = [0.12, 0.72, 0.40, 1.0];
const COLOR_PANEL_DEFAULT: [number, number, number, number] = [
  0.018, 0.026, 0.039, 0.82,
];
const COLOR_TEXT: [number, number, number, number] = [0.82, 0.95, 0.98, 1.0];
const SPACING_S = 4;
const SPACING_M = 8;

// Each tab's (key, label). Lexicographic key order is what `Switch` expands in,
// so the content panels and the tab strip stay in a stable, cross-runtime order.
const TABS: ReadonlyArray<[string, string]> = [
  ["loadout", "LOADOUT"],
  ["stats", "STATS"],
];

/** A simple content panel for the demo — distinct text per tab. */
function panel(title: string, body: string) {
  return VStack({ gap: SPACING_S, padding: SPACING_M }, [
    Text({ content: title, fontSize: 22, color: COLOR_OK }),
    Text({ content: body, fontSize: 16, color: COLOR_TEXT }),
  ]);
}

/**
 * Build the tabs-demo registrations: the named `cellWrite` reactions (the tab
 * `onPress` targets) and the `alwaysOn` UI tree. Spread into `setupLevel`'s
 * returned manifest.
 */
export function tabsDemo(): {
  reactions: NamedReactionDescriptor[];
  uiTrees: UiTreeRegistration[];
} {
  // One string-valued presentation cell selects the active tab.
  const sel = ui.createLocalState({ tab: "loadout" });

  // Each tab's onPress is a named `cellWrite` reaction setting the cell. A
  // Button `onPress` resolves a named reaction — so wrap `cells.tab.set(key)`
  // (a `cellWrite` descriptor) in `defineReaction` and reference it by name.
  const reactions: NamedReactionDescriptor[] = TABS.map(([key]) =>
    defineReaction(`tabsDemo_select_${key}`, sel.cells.tab.set(key)),
  );

  // One tab button: predicate `bind` → styleRanges highlight; SAME predicate as
  // `selected` (a11y); `role:"tab"`; onPress sets the cell. Highlight and a11y
  // are one expression, so they never desync.
  const tab = ([key, label]: [string, string]) =>
    Button({
      id: `tabsDemo-${key}`,
      label,
      role: "tab",
      // Highlight: the predicate resolves to 0/1; styleRanges recolors the band.
      bind: sel.cells.tab.is(key),
      styleRanges: {
        max: 1,
        entries: [{ upTo: 0, color: COLOR_PANEL_DEFAULT }, { color: COLOR_OK }],
      },
      // A11y: the SAME predicate tags the button selected when active.
      selected: sel.cells.tab.is(key),
      onPress: `tabsDemo_select_${key}`,
    });

  const tabsDemoTree = defineUiTree({
    name: "tabsDemo",
    alwaysOn: true,
    tree: Tree(
      { anchor: "topRight", offset: [-16, 16], role: "group", accessibleName: "Tabs demo" },
      // `localState` is declared on the vstack (NOT the tablist strip / Grid) so
      // the cell scope covers both the tab strip and the content `Switch`.
      VStack(
        {
          localState: sel.scope,
          gap: SPACING_M,
          padding: SPACING_M,
          fill: [0.05, 0.05, 0.08, 0.85],
        },
        [
          HStack({ role: "tablist", gap: SPACING_S, focus: "linear" }, TABS.map(tab)),
          // `Switch` expands to each panel with `visibleWhen: cell.is(key)` injected
          // in sorted key order — exactly the active tab's panel is visible.
          VStack(
            { gap: SPACING_S },
            Switch(sel.cells.tab, {
              loadout: panel("Loadout", "Pistol · Shotgun · Plasma"),
              stats: panel("Stats", "Kills 0 · Accuracy --"),
            }),
          ),
        ],
      ),
    ),
  });

  return {
    reactions,
    uiTrees: [tabsDemoTree],
  };
}
