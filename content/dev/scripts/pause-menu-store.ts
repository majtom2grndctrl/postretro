import { defineStore } from "postretro";

// M13 Goal F (Task 5) demo store: the `audio.master` namespace the engine
// pause-menu demo binds. The pause menu's volume slider binds `audio.master`
// and steps it through the existing `setState` wire; the engine's App-side
// audio-master consumer reads the amplitude each frame and applies it
// (amplitude → dB) to the audio main bus, so the slider audibly changes volume.
//
// `audio.master` is a mod-declared, WRITABLE Number slot (amplitude in `[0, 1]`,
// default unity `1.0`). It is NOT engine-owned — the engine only consumes it; the
// slot's existence is content. `persist: false` keeps it out of the save file
// (a settings-menu save path is a later goal). Declared at mod init so the
// namespace is registered before the slider's first write — an absent
// slot would make the write log-and-skip and leave the volume at unity.
//
// `defineStore` is a pure builder. `setupMod` returns
// `pauseMenuStore.declaration` through `stores`; importing this module alone
// does not register anything. See: context/lib/scripting.md §5.
export const pauseMenuStore = defineStore("audio", {
  master: { type: "number", default: 1.0, range: [0.0, 1.0], persist: false },
});
