--!strict
-- REFERENCE CONTENT — Luau twin of `faction-sentiment.ts`.
--
-- This module intentionally mirrors the TS fixture's descriptors so a Luau
-- mod can demonstrate the same sympathetic-backstab-brawl loop.

local ReferenceEnemy = require("./reference-enemy.luau")

local M = {}

M.SENTIMENT_CABAL_FACTION = "sentiment.cabal"
M.SENTIMENT_RESISTANCE_FACTION = "sentiment.resistance"
M.SENTIMENT_CABAL_CLASSNAME = "faction_sentiment_cabal"
M.SENTIMENT_RESISTANCE_CLASSNAME = "faction_sentiment_resistance"
M.SENTIMENT_TOLERANCE = 4
M.SENTIMENT_DECAY = 0.25

local HARM_SENTIMENT_DELTA = -0.2

local function sentimentEnemy(canonicalName: string, faction: string)
  return defineEntity({
    canonicalName = canonicalName,
    components = {
      faction = faction,
      tolerance = M.SENTIMENT_TOLERANCE,
      health = ReferenceEnemy.referenceEnemyEntity.components.health,
      mesh = ReferenceEnemy.referenceEnemyEntity.components.mesh,
      behavior = ReferenceEnemy.referenceEnemyEntity.components.behavior,
    },
  })
end

M.factionSentimentCabalEntity = sentimentEnemy(
  M.SENTIMENT_CABAL_CLASSNAME,
  M.SENTIMENT_CABAL_FACTION
)
M.factionSentimentResistanceEntity = sentimentEnemy(
  M.SENTIMENT_RESISTANCE_CLASSNAME,
  M.SENTIMENT_RESISTANCE_FACTION
)

M.factionSentimentBackstab = defineImpactEvent(
  "faction-sentiment.backstab",
  { tag = "faction-sentiment-target" },
  function(impact)
    return {
      {
        when = impact.amount:gt(0),
        ["do"] = {
          impact.target:adjustSentimentToward(impact.source, impact.amount:times(HARM_SENTIMENT_DELTA)),
          impact.source:adjustSentimentToward(impact.target, impact.amount:times(HARM_SENTIMENT_DELTA)),
        },
      },
    }
  end
)

M.factionSentimentStoryBeat = defineReaction(
  "faction-sentiment.story-beat",
  setSentiment(M.SENTIMENT_CABAL_FACTION, M.SENTIMENT_RESISTANCE_FACTION, -1)
)
M.factionSentimentPlayerChoice = defineReaction(
  "faction-sentiment.player-choice",
  adjustSentiment(M.SENTIMENT_RESISTANCE_FACTION, M.SENTIMENT_CABAL_FACTION, -0.25)
)
M.factionSentimentReactions = {
  M.factionSentimentStoryBeat,
  M.factionSentimentPlayerChoice,
}
M.factionSentimentTriggerEvents = {
  onTriggerEvent(
    { tag = "faction_sentiment_story" },
    "enter",
    { M.factionSentimentStoryBeat, M.factionSentimentPlayerChoice }
  ),
}

return M
