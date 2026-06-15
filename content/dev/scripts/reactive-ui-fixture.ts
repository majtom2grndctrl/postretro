// M13 G2 author-facing fixture — reactive UI + a11y prop narrowing.
//
// This file is a documented REVIEW GATE, not a runtime script. The repo has no
// `tsc` CI, so the `@ts-expect-error` lines below assert the intended compile-
// time behavior for a human reviewer (and for an author who opens it in an
// IDE): each marked line MUST be a type error. If a future change makes one of
// them compile cleanly, `tsc --noEmit` would flag the now-unused
// `@ts-expect-error` — the narrowing has regressed and the review gate fails.
//
// The positive (non-`@ts-expect-error`) lines are the canonical, well-typed
// authoring shapes the tabs demo (`tabs-demo.ts`) is built from.
//
// See: context/lib/scripting.md §7, context/lib/ui.md §4, docs/scripting-
// reference.md "Reactive UI", M13 G2 Task 5.

import {
  Announce,
  Bar,
  Button,
  Image,
  Slider,
  Text,
  ui,
} from "postretro";

// --- Shared cell scope ------------------------------------------------------
// A string-valued presentation cell. `cells.tab` is a `LocalStateHandle<string>`;
// `.is(v)` returns a `Predicate` with the comparand typed to the cell's value
// type (here `string`).
const sel = ui.createLocalState({ tab: "loadout" });

// --- (1) Per-kind prop narrowing: `content` is Text-only --------------------
// `content` is a `Text` prop. The interactive/passive widget prop types carry
// no `content`, so wiring it onto a Button/Slider/Bar is an excess-property
// (unknown-prop) type error — the headline per-kind narrowing the AC requires.
const _text = Text({ content: "Loadout", fontSize: 18, color: "ok" });

// @ts-expect-error — `content` is a Text-only prop; ButtonProps has no `content`.
const _badButtonContent = Button({ id: "x", label: "X", onPress: "noop", content: "X" });

// --- (2) Interactive widgets require an accessible name (label xor labelledBy)
// A `Button` is interactive: exactly one of `label` / `labelledBy` is required.
const _named = Button({ id: "ok", label: "OK", onPress: "noop" });
const _labelled = Button({ id: "ok2", labelledBy: "title", onPress: "noop" });

// An unnamed interactive widget does not satisfy the name-XOR union — neither
// `label` nor `labelledBy` is present, so the props object is not assignable.
// @ts-expect-error — interactive Button requires `label` xor `labelledBy`.
const _unnamedButton = Button({ id: "bad", onPress: "noop" });

// --- (3) A passive widget (`Bar`) needs NO name -----------------------------
// `Bar` is passive (not focusable); its prop type carries no name requirement,
// so this compiles with no `label`/`labelledBy`.
const _bar = Bar({
  bind: { slot: "player.health" as never },
  max: 100,
  fill: "ok",
  background: [0.1, 0.1, 0.1, 1],
});

// --- (4) Image: label xor decorative ----------------------------------------
// `Image` requires exactly one of `label` (alt text) or `decorative: true`.
const _altImage = Image({ asset: "ui/portrait", label: "Player portrait" });
const _decoImage = Image({ asset: "ui/divider", decorative: true });

// Neither label nor decorative: not assignable to the XOR union.
// @ts-expect-error — Image requires `label` xor `decorative: true`.
const _badImage = Image({ asset: "ui/orphan" });

// --- (5) `.is(v)` comparand is typed to the cell/slot value type -------------
// `cells.tab` is a string cell, so `.is("...")` type-checks and `.is(<number>)`
// does not. This is the typed predicate-helper narrowing the AC requires.
const _selPredicate = sel.cells.tab.is("loadout"); // ok: string comparand

// @ts-expect-error — `tab` is a string cell; `.is()` comparand must be a string.
const _badIs = sel.cells.tab.is(3);

// --- (6) The full tab-button shape (well-typed, the demo's building block) ---
// A tab carries: a `Predicate` `bind` (drives the styleRanges highlight), the
// SAME predicate as `selected` (a11y), and `onPress` setting the cell. `role`
// annotates it as a tab. All of these are well-typed together.
const _tab = Button({
  id: "t-loadout",
  label: "Loadout",
  role: "tab",
  bind: sel.cells.tab.is("loadout"),
  styleRanges: {
    max: 1,
    entries: [{ upTo: 0, color: "panel.default" }, { color: "ok" }],
  },
  selected: sel.cells.tab.is("loadout"),
  onPress: "selectLoadout",
});

// --- (7) Announce: text is positional, priority narrows ---------------------
const _announce = Announce({ priority: "assertive" }, "Loadout tab selected");

// @ts-expect-error — `priority` is "polite" | "assertive", not an arbitrary string.
const _badAnnounce = Announce({ priority: "loud" }, "nope");

void _text;
void _badButtonContent;
void _named;
void _labelled;
void _unnamedButton;
void _bar;
void _altImage;
void _decoImage;
void _badImage;
void _selPredicate;
void _badIs;
void _tab;
void _announce;
void _badAnnounce;
