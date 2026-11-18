# E23--text-field-control

Brief · compact · Epic 23 · reads: `context/lib/ui.md` §4, §4.2 · `context/lib/input.md` §5 · read at 44230810

## Problem
The owner raised it after a dev-frontend UX pass (`done/coop--in-app-host-and-join/plan.md` §After review names this follow-up). Basis: an observed defect and a requested capability. On the Host and Join menus, a text field is three widgets in a row: a caption, the value in a dark well, and an EDIT button. Only EDIT takes a click, so a click on the caption or the value does nothing. The cause: a `Button` draws only its label and cannot hold other widgets. A boxed control is therefore a container fill around a label-sized button, so its hit rect and focus ring are the label's, inset from the box. A row cannot be one target. When done, a field is one control. A click anywhere on its row, gamepad confirm or Enter opens the keyboard on that field. The row is one focus stop, its ring frames the whole row, and its accessible name, role and value are ready for the screen-reader snapshot: "Password, edit text, \*\*\*\*\*\*\*". Any mod can make a whole box one control.

## Decisions
- **A Button may hold passive content.** A Button takes one optional content widget. With content, it draws its content in place of its label, and its hit rect and focus rect are exactly the content's drawn box: no wider when its parent stretches, no narrower than the box's padding. Without content, a Button is unchanged on the wire and on screen. Layer: engine mechanism, in the descriptor, both script bridges, both SDKs and the UI crate; row layout, captions and the pencil are dev-mod policy. One content root, not a children list, so a Button gains no layout props: the stack vocabulary inside it sets the box.
- **Content is never focusable.** A Button or Slider anywhere in a Button's content is an error, and so is a container that declares a focus policy there. Both SDK factories throw at construction, naming the button and the offending widget. The JS and Luau bridges and the engine's JSON tree loader reject the same tree with a named error, and the tree is skipped as any malformed tree is (`drain_ui_trees_js`, `drain_ui_trees_lua`, `tree_asset.rs`). This keeps `ui.md` §4's rule that focus stops are interactive widgets only, and keeps nested interactive content out of the tree.
- **A content Button keeps its name rules and drops its label styling.** Exactly one of `label` or `labelledBy` stays required (`validate_interactive_name`). With content, `label` only names the button. `valueText`, `bind` and `styleRanges` style or replace the label run a content Button no longer draws, so authoring any of them with content is an error in both SDKs and both bridges.
- **A field's value comes from `valueFrom`.** `valueFrom` names a `Text` inside the button's content, as `labelledBy` names a node. It is required to resolve inside that content; outside it, or on a Button without content, it is an error. The value is that Text's resolved string. A masked Text resolves to its mask (`masked` in `tree/bindings.rs`), so a password's value is never its content (`ui.md` §4.2). This diverges from the stated direction, which named `valueText`: `valueText` cases hold fixed strings chosen by predicates and cannot carry typed text. See Open questions.
- **New role: `textField`.** It joins `Role` and both SDKs' role lists, and its wire form is `"textField"`. It is descriptor vocabulary only: no engine behavior keys on it, and activation stays the button's `onPress`. The platform role it maps to is U4's.
- **This brief adds the accessible-facts read, not the snapshot.** For a focus stop it yields name, role and value. Name is the inline `label`, else the resolved text of the `labelledBy` node. Role is the authored role, else `implicit_role`. Value is the `valueFrom` Text's resolved text, else the showing `valueText` case. Nothing resolves `labelledBy` today, and U4's snapshot calls this read. Snapshot assembly, the adapter, live regions and static-text nodes stay U4's.
- **A Button's content is presentational to assistive technology.** Text and images inside it never become their own snapshot nodes; the button's name and value carry them, as ARIA treats button children. Recorded in `ui.md` §4.2 at promotion; U4's rows prove it.
- **The dev frontend's fields become content Buttons.** `field` and `passwordField` in `content/dev/scripts/frontend-menu.ts` build one Button per row, captioned by `labelledBy`, valued by `valueFrom`, with role `textField` and the field's existing edit reaction (`openTextEntry(undefined, target)` through `editFieldSteps`). The EDIT button goes. A small decorative pencil inside the well marks the row as editable. Ids stay `joinAddress`, `joinPassword` and `hostPassword`, so tests, `initialFocus` and restore on return hold.
- **Rejected: a pointer-only `label for` hit extension.** A caption or well would forward clicks to EDIT, which stays the focus stop. It is content-only and cheap, but it helps only mouse users, keeps the extra control, and leaves the inset ring and padding miss in every boxed button.
- **Non-goals.**
  - The snapshot, its adapter, the platform role mapping, and typing on a focused field to open entry: U4.
  - Moving the dev frontend's boxed `action` buttons onto content Buttons. It changes how their label color and highlight are authored (UX gaps 3 and 4), so it is its own pass.
  - Button fill, padding or variant props and a disabled look. Content covers the box; theme-token visuals are U2's.
  - Content on `Slider`, and in-place editing with a caret.
- **Docs owed at promotion.** `ui.md` §4: a Button's content is passive and its box is its stop; the dev grammar's field line and its "a button draws only its label" sentence. `ui.md` §4.2: `textField`, `valueFrom`, and presentational content. `docs/scripting-reference.md` §Interactive widgets and §`valueText`.

### Scripting surface
```ts
// One control: the whole row is the button's box.
Button(
  {
    id: "joinPassword",
    labelledBy: "joinPasswordCaption", // name: "Password"
    valueFrom: "joinPasswordValue", // value: the mask, never the password
    role: "textField",
    onPress: editPassword, // defineReaction(..., editFieldSteps(session.joinPassword, ...))
  },
  HStack({ gap: 10, align: "center" }, [
    VStack({ width: 84 }, [Text({ id: "joinPasswordCaption", content: "Password", fontSize: 14, color: COLOR_MUTED })]),
    HStack({ width: 220, padding: 8, gap: 0, align: "center", fill: FILL_WELL }, [
      Text({ id: "joinPasswordValue", content: "", fontSize: 15, mask: true, bind: bindState(session.joinPassword) }),
      Text({ content: "None", fontSize: 15, color: COLOR_MUTED, visibleWhen: stateEquals(session.joinPassword, "") }),
    ]),
  ]),
);
```
Luau mirrors it: `Button({ ..., valueFrom = "joinPasswordValue", role = "textField" }, HStack({ ... }, { ... }))`. Both ship.

## Acceptance
### Automated
**Whole-row target** (Join and Host menus at 1280×720, through the pointer path: click, hit test, activation)
- [ ] A click at the centre of the caption, of the value well and of the pencil each opens the keyboard on that field: for Address, titled "HOST ADDRESS" and holding the field's text; for each password, masked. Today the caption and the well open nothing.
- [ ] A click 1 px outside the row's box on each side opens nothing.
- [ ] A content Button whose box is a filled, padded stack inside a stretching parent: a click on the padding activates it, and its stop rect equals the box's drawn rect, never the parent's stretched width.
**One stop, whole-box ring**
- [ ] The Join menu's stops are exactly `joinAddress`, `joinPassword`, `joinStart` and the way back, in that order; the Host menu's are `hostPassword`, `hostStart` and the way back. No "EDIT" text is drawn.
- [ ] Each field's stop rect contains its caption's and its well's drawn rects, and the focus ring drawn for it lies outside the row's box on all four sides.
**Activation**
- [ ] With a field focused, gamepad confirm and hardware Enter each open the keyboard on that field. Committing writes that field's slot only, and focus returns to the field.
**Validation, both SDKs and both bridges**
- [ ] A Button with a Button or a Slider at any depth in its content, or with a focus-policy container there, throws in the TS factory and errors in the Luau factory, naming the button and the offending widget. The same tree authored as raw descriptor data, past the factory, is rejected by the JS bridge, the Luau bridge and the engine JSON loader with a named error; the tree is skipped and the rest of the manifest loads.
- [ ] Each of `valueText`, `bind` and `styleRanges` with content, `valueFrom` without content, and a `valueFrom` that names no Text inside the content, is rejected the same four ways.
- [ ] Both bridges read `content`, `valueFrom` and `role: "textField"` onto the descriptor, and the TS and Luau factories emit byte-identical wire for the Scripting surface example. A Button without content emits the same bytes as before.
**Accessible facts**
- [ ] Join Address holding "10.0.0.2": name "Address", role `textField`, value "10.0.0.2"; blank: value "".
- [ ] Join and Host password holding "hunter2": name "Password", role `textField`, value "\*\*\*\*\*\*\*"; the read's output never contains "hunter2".
- [ ] A label-only toggle with `valueText` (the engine accessibility panel's): its value is the showing case's text, and its name is its `labelledBy` text.
**Dev frontend**
- [ ] `every_dev_frontend_screen_is_one_layer_with_one_way_back`, `the_dev_session_menus_show_one_state_at_a_time`, `a_menu_text_field_commits_into_its_own_slot`, `the_dev_keyboard_titles_its_field_and_masks_a_password` and `a_title_notice_reopens_the_field_to_fix` pass, and the dev fields are built with the Scripting surface shape.
### Manual
- [ ] Dev frontend, Host and Join, in a 16:9 window and a tall one: clicking a field's caption or value opens the keyboard. The ring frames the whole row with mouse, keyboard and gamepad. The pencil shows in each well and never takes focus.

## Path
- **Seams.** `ButtonWidget` and `Role` (`scripting-core` `ui/descriptor/widgets.rs`, `accessibility.rs`). `button_widget_from_js` and its Luau twin (`data_descriptors/{js,lua}/ui_widgets.rs`); one shared content validator called by both bridges and by `tree_asset.rs`, which loads through serde with no bridge. `build_button` (`crates/ui/src/tree/build.rs`) builds a text leaf today; `build_slider` is the precedent for one stop over a composite node. Hit and ring already follow the stop rect: `collect_focus_node` projects the button's node, `hit_test_topmost` hits it, and `push_focus_ring` frames it.
- **Every walk must enter content.** `widget_children` (`tree/widget_meta.rs`) feeds the lockstep focus, draw, binding and visibility walks. Private child walks also exist: `collect_image_keys` (`image_refs.rs`), the copy in `presentation_cells.rs`, `validate_widget_sources` (`runtime_manifest.rs`), `contains_glyph` (`app/template_glyphs.rs`), and the `Widget::Button` sites in `main.rs`. A missed one shows as an unloaded pencil image, a dead bind or an unresolved glyph inside a button.
- **Shape.** The content root's node is the button's node, or a wrapper that sizes exactly to it, so the stop rect is the box. Rival: Button gains stack props (`fill`, `padding`, `gap`, `width`) and a children list; it mirrors every future stack prop onto Button.
- **Accessible-facts read.** Resolve names and values from the built tree's resolved text by authored id. Keep it out of the per-frame focus export, so no string is built per stop per frame (cf. `in-progress/ui-text-prepare-churn`).
- **First slice.** A content Button in a padded box: its stop rect equals the box, and a pointer click on the padding activates it. This falsifies the riskiest assumption, that the lockstep descriptor and taffy walks accept a Button with descendants.
- **Tests.** Bridge rows follow `data_descriptors/tests/g2_fields.rs`; Luau factory rows follow `ui_bridge_drains.rs`, which loads `sdk/lib/ui/widgets.luau`. Typedef templates (`typedef/templates/sdk_lib.d.ts`, `sdk_lib.luau`, `ui_sdk_module.d.ts`) gain the props.
- **Split first.** `scripting-core` `ui/descriptor/widgets.rs`, `sdk/lib/ui/widgets.{ts,luau}`, `content/dev/scripts/frontend-menu.ts` and `crates/postretro/src/startup/session_entry_tests.rs` are past ~800 lines. Split those this change extends, behavior-preserving, each in its own commit; new frontend tests go in a sibling file.

## Open questions
- Confirm `valueFrom`, a reference to a Text inside the content, over extending `valueText` with a bound case. The direction named `valueText`, which holds only fixed strings today. — owner — **blocks build**
- The pencil's form: a decorative image in the dev mod's UI images, or a text glyph the font draws. — **delegated**: the executor decides and reports it in the plan of record

## Boundary inventory
Both runtimes ship every row.
| Name | Rust | Wire / serde | JS / TS | Luau | FGD KVP |
|---|---|---|---|---|---|
| Button content | `content: Option<Box<Widget>>` on `ButtonWidget` | `"content": { widget }` on `button`, omitted when absent | `Button(props, content?)` | `Button(props, content?)` | n/a |
| Value source | `value_from: Option<String>` on `ButtonWidget` | `"valueFrom"`, omitted when absent | `valueFrom?: string` | `valueFrom` | n/a |
| Text-field role | `Role::TextField` | `"textField"` | `"textField"` in `WidgetRole` | `"textField"` | n/a |
