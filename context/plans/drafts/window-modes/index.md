# window-modes

Seed · needs `/draft-session` before a brief · read at 87c65ac24

## Problem
A player can only play in a window. There is no fullscreen of any kind. The developer wants window-mode options; this is a requested capability, split out of `hidpi-render-scale` because it raises questions that work does not need.

## Owner decisions so far
- Three modes: windowed, borderless fullscreen, and exclusive fullscreen with a display-mode picker.
- First launch stays windowed.
- On macOS, borderless is the recommended fullscreen choice. Exclusive fullscreen there switches the display mode and locks out Spaces and task switching.
- The engine reads the actual mode back from the window. The macOS green button and OS transitions change the mode without engine involvement.

## Direction-review advice to weigh
Build windowed and borderless first. Add exclusive only if Windows testing makes the case for it.

## Questions the session must settle
- **Boot order.** Player options load after the first pixels (`boot_sequence.md`). A saved fullscreen mode must either switch mid-splash on every launch or be read narrowly before the window exists. The second is a divergence from the documented boot sequence.
- **E23 U4.** The ready plan `E23--accessibility` creates the window hidden for the `accesskit_winit` adapter. Window-mode application must order correctly against that reveal.
- **Option surface.** A `PlayerOptions` field needs a window-side chokepoint beside the render profile, because a mode change is a winit call, not a renderer setter. The `options.*` slot is a scripting-surface change.
- **Proof.** Mode switching needs a manual check on both macOS and Windows.

## Grounding already gathered
`ready/hidpi-render-scale/research.md` §winit 0.30.13 platform notes covers fullscreen variants, video-mode enumeration, the macOS caveats and the transition queueing.
