import { Text, getGameState, stateCompare, stateEquals } from "postretro/ui";

const { player } = getGameState();

const reviving = Text({
  content: "REVIVING",
  visibleWhen: stateCompare(player.assistProgress, "greaterThan", 0),
});
const low = stateCompare(player.health, "atMost", 25);
// @ts-expect-error A numeric comparison needs a Number state ref.
const onString = stateCompare(player.lifeState, "greaterThan", 0);
// @ts-expect-error Ops are greaterThan, lessThan, atLeast, atMost.
const badOp = stateCompare(player.health, "gt", 0);
// @ts-expect-error A predicate carries `equals` or `compare`, never both.
const both = Text({ content: "x", visibleWhen: { ...stateEquals(player.lifeState, "down"), compare: { op: "atLeast", value: 1 } } });

export { reviving, low, onString, badOp, both };
