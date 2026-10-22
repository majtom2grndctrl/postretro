import { defineReaction } from "postretro";
// @ts-expect-error `setGhostAudio` lives in `postretro/audio`, not the root module.
import { setGhostAudio as fromRoot } from "postretro";
import { setGhostAudio, type GhostAudioFields } from "postretro/audio";

const fields: GhostAudioFields = { volume: 0.9, lowPassHz: 5000, periodMs: 1500 };
// A target-less body is a whole reaction body, like `setSentiment`.
const swell = defineReaction("echo.swell", setGhostAudio(fields));
// @ts-expect-error An unknown Ghost bus field.
const unknown = setGhostAudio({ gain: 1 });

export { fromRoot, swell, unknown };
