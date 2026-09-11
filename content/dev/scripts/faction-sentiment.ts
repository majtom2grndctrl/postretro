// REFERENCE CONTENT — mutable faction sentiment.
//
// Place one `faction_sentiment_cabal` and one `faction_sentiment_resistance`
// with the `faction-sentiment-target` tag. They begin sympathetic, so the
// faction-crossfire offer floor leaves them alone. A damaging impact then moves
// both directed pairs below zero, making each peer an ordinary hostile offer.

import {
  defineEntity,
  defineImpactEvent,
  defineReaction,
  onTriggerEvent,
} from "postretro";
import type {
  EntityTypeDescriptor,
  NamedReactionDescriptor,
  TriggerEventDescriptor,
} from "postretro";
import { adjustSentiment, setSentiment } from "postretro/ui";
import { referenceEnemyEntity } from "./reference-enemy";

export const SENTIMENT_CABAL_FACTION = "sentiment.cabal";
export const SENTIMENT_RESISTANCE_FACTION = "sentiment.resistance";

export const SENTIMENT_CABAL_CLASSNAME = "faction_sentiment_cabal";
export const SENTIMENT_RESISTANCE_CLASSNAME = "faction_sentiment_resistance";

// The relationship rows own the fallback tolerance; these matching archetype
// values make the fixture's retaliation facts equally legible when it brawls.
export const SENTIMENT_TOLERANCE = 4;
export const SENTIMENT_DECAY = 0.25;
const HARM_SENTIMENT_DELTA = -0.2;

const referenceBehavior = referenceEnemyEntity.components.behavior;
if (referenceBehavior === undefined || referenceBehavior === null) {
  throw new Error("reference_enemy must retain its behavior graph for the sentiment fixture");
}

function sentimentEnemy(canonicalName: string, faction: string): EntityTypeDescriptor {
  return defineEntity({
    canonicalName,
    components: {
      ...referenceEnemyEntity.components,
      faction,
      tolerance: SENTIMENT_TOLERANCE,
      behavior: referenceBehavior,
    },
  });
}

export const factionSentimentCabalEntity = sentimentEnemy(
  SENTIMENT_CABAL_CLASSNAME,
  SENTIMENT_CABAL_FACTION,
);

export const factionSentimentResistanceEntity = sentimentEnemy(
  SENTIMENT_RESISTANCE_CLASSNAME,
  SENTIMENT_RESISTANCE_FACTION,
);

// Harm is directional at the primitive surface. Chain both handles explicitly:
// the victim sours on its attacker, then the attacker sours on the victim.
// A standard 10-point reference-weapon hit moves the sympathetic 0.25 baseline
// to -1.75, which is hostile before the same tick's AI offer scan.
export const factionSentimentBackstab = defineImpactEvent(
  "faction-sentiment.backstab",
  { tag: "faction-sentiment-target" },
  ({ target, source, amount }) => [
    {
      when: amount.gt(0),
      do: [
        target.adjustSentimentToward(source, amount.times(HARM_SENTIMENT_DELTA)),
        source.adjustSentimentToward(target, amount.times(HARM_SENTIMENT_DELTA)),
      ],
    },
  ],
);

// Map authors can place a trigger volume tagged `faction_sentiment_story` to
// stage the plot beat. Both writes remain frame-end consequential reactions:
// they become visible to AI on the following fixed tick, unlike the impact.
export const factionSentimentStoryBeat = defineReaction(
  "faction-sentiment.story-beat",
  setSentiment(SENTIMENT_CABAL_FACTION, SENTIMENT_RESISTANCE_FACTION, -1),
);

// This separate reaction is suitable for a host-evaluated choice source. It
// demonstrates that an adjustment starts at the live value rather than the
// authored baseline.
export const factionSentimentPlayerChoice = defineReaction(
  "faction-sentiment.player-choice",
  adjustSentiment(SENTIMENT_RESISTANCE_FACTION, SENTIMENT_CABAL_FACTION, -0.25),
);

export const factionSentimentReactions: NamedReactionDescriptor[] = [
  factionSentimentStoryBeat,
  factionSentimentPlayerChoice,
];

export const factionSentimentTriggerEvents: TriggerEventDescriptor[] = [
  onTriggerEvent(
    { tag: "faction_sentiment_story" },
    "enter",
    [factionSentimentStoryBeat, factionSentimentPlayerChoice],
  ),
];
