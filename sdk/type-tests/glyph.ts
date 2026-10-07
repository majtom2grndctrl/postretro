import { Glyph, HStack, Text } from "postretro/ui";

const row = HStack({ gap: 8 }, [Glyph({ command: "nav_confirm" }), Text({ content: "SELECT" })]);
// @ts-expect-error A glyph names an engine command ID.
const unknown = Glyph({ command: "grapple" });

export { row, unknown };
