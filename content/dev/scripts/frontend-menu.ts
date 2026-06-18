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
    name: "Campaign Test",
    tags: ["campaign", "recommended"],
  },
  {
    id: "campaign-test-mtex-002",
    path: "maps/campaign-test--0.02-mtex.prl",
    name: "Campaign Test (mtex 0.02)",
    tags: ["campaign", "lighting", "variant"],
  },
  {
    id: "combat-demo",
    path: "maps/combat-demo.prl",
    name: "Combat Demo",
    tags: ["combat", "recommended"],
  },
  {
    id: "occlusion-test",
    path: "maps/occlusion-test.prl",
    name: "Occlusion Test",
    tags: ["visibility", "lighting", "recommended"],
  },
  {
    id: "occlusion-test-mtex-001",
    path: "maps/occlusion-test--0.01-mtex.prl",
    name: "Occlusion Test (mtex 0.01)",
    tags: ["visibility", "lighting", "variant"],
  },
  {
    id: "occlusion-test-mtex-0015",
    path: "maps/occlusion-test--0.015-mtex.prl",
    name: "Occlusion Test (mtex 0.015)",
    tags: ["visibility", "lighting", "variant"],
  },
  {
    id: "occlusion-test-mtex-002",
    path: "maps/occlusion-test--0.02-mtex.prl",
    name: "Occlusion Test (mtex 0.02)",
    tags: ["visibility", "lighting", "variant"],
  },
  {
    id: "occlusion-test-shadow-resolution",
    path: "maps/occlusion-test--shadow-resolution-test.prl",
    name: "Occlusion Test (shadow resolution)",
    tags: ["visibility", "lighting", "shadow", "variant"],
  },
  {
    id: "test-animated-weight-maps-cap",
    path: "maps/test_animated_weight_maps_cap.prl",
    name: "Animated Weight Maps: Cap",
    tags: ["lighting", "animated-weight-map", "test"],
  },
  {
    id: "test-animated-weight-maps-mixed",
    path: "maps/test_animated_weight_maps_mixed.prl",
    name: "Animated Weight Maps: Mixed",
    tags: ["lighting", "animated-weight-map", "test"],
  },
  {
    id: "test-animated-weight-maps-occluded",
    path: "maps/test_animated_weight_maps_occluded.prl",
    name: "Animated Weight Maps: Occluded",
    tags: ["lighting", "animated-weight-map", "test"],
  },
  {
    id: "test-animated-weight-maps-single",
    path: "maps/test_animated_weight_maps_single.prl",
    name: "Animated Weight Maps: Single",
    tags: ["lighting", "animated-weight-map", "test"],
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

function mapsTaggedWithout(tag: string, excludedTag: string): ModMapEntry[] {
  return mapCatalog.filter((entry) => hasTag(entry, tag) && !hasTag(entry, excludedTag));
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
          section("Visibility", mapsTaggedWithout("visibility", "variant")),
        ]),
        VStack({ gap: 14, align: "stretch" }, [
          section("Animated Weight Maps", mapsTagged("animated-weight-map")),
          section("Bake Variants", mapsTagged("variant")),
        ]),
      ],
    ),
  ),
});

export const frontendReactions: NamedReactionDescriptor[] = [...frontendStartReactions];
