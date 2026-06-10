import { defineStore } from "postretro";

// M13 Goal C demo store: the `intro` namespace the demo HUD's flash panel binds
// (`intro.flashColor`) and the static UI proxy writes during level load. Declared
// at mod init so the namespace is registered before the proxy's first write — an
// absent namespace would make the proxy's write skip-with-warn and the panel fall
// back to its literal fill.
//
// `flashColor` is a length-4 linear-RGBA array (`[r, g, b, a]`). `persist: false`
// keeps it out of the save file — it is a transient level-load animation slot,
// not durable player state. Wire casing is camelCase across TS/JS/Luau
// (`flashColor`, `type`, `default`, `persist`); see the Luau parity reference in
// `intro-store.luau`.
//
// `defineStore` is a `DefinitionOnly` primitive callable during mod init; this
// helper is invoked for its side effect (namespace registration) from
// `setupMod`, not for a return value. See: context/lib/scripting.md §3.
export function registerIntroStore(): void {
  defineStore("intro", {
    flashColor: { type: "array", default: [0.0, 0.65, 0.75, 1.0], persist: false },
  });
}
