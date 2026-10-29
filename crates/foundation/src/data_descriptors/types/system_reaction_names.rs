// The wire names of every system reaction, for consumers below `entities`.
// See: context/lib/scripting.md §10.4

/// Every system reaction primitive. `entities`' `SystemReactionKind::ALL` is
/// the source of truth; its drift test keeps this list equal to it. The
/// script compiler's light pass reads it to admit a system sequence step.
pub const SYSTEM_REACTION_PRIMITIVES: [&str; 19] = [
    "playSound",
    "rumble",
    "flashScreen",
    "vignette",
    "screenShake",
    "showDialog",
    "openMenu",
    "closeDialog",
    "loadLevel",
    "restartLevel",
    "returnToFrontend",
    "setState",
    "setSentiment",
    "adjustSentiment",
    super::ghost_audio::SET_GHOST_AUDIO,
    "cellWrite",
    "appendText",
    "backspaceText",
    "clearText",
];
