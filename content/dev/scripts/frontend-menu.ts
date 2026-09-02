import {
  defineMapCatalog,
  defineReaction,
  type ModMapEntry,
  type NamedReactionDescriptor,
} from "postretro";
import { Button, HStack, Text, Tree, VStack, defineUiTree, loadLevel } from "postretro/ui";

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

export const frontendMenu = defineUiTree({
  name: "frontend.devLevelSelect",
  tree: Tree(
    {
      anchor: "center",
      offset: [0, 0],
      captureMode: "capture",
      initialFocus: `start-${mapCatalog[0].id}`,
      accessibleName: "Dev level select",
      role: "group",
    },
    HStack(
      {
        gap: 18,
        padding: 18,
        align: "start",
        fill: [0.018, 0.026, 0.039, 0.94],
        focus: { policy: "linear", wrap: true },
      },
      [
        VStack({ gap: 14, align: "stretch" }, [
          section("Recommended", mapsTagged("recommended")),
          section("Development Tests", mapsTagged("test")),
        ]),
      ],
    ),
  ),
});

export const frontendReactions: NamedReactionDescriptor[] = [...frontendStartReactions];
