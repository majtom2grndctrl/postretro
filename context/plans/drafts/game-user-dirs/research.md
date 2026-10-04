# game-user-dirs — research

## Ordering pins

| id | Scenario | Ordering | Expected outcome |
|---|---|---|---|
| P1 | `postretro/` already holds settings and state; first launch with `--app-name g` | stage-1 resolution → `Session::build` loads settings → `player_id` generated → first-launch save | Defaults and a new `player_id` under `g`; the accessibility first-launch hold shows again; nothing read from or written to `postretro/` |
| P2 | Pre-window settings read (`ready/window-modes`) followed by `Session::build` and the state store | one resolution at stage 1, consumed by every later read and write | Settings load path = settings save path = state-store directory; no consumer resolves a second time |
| P3 | `--app-name a --app-name b` | stage-1 scan, first occurrence wins (as `--mod`) | Directories resolve under `a` |
| P4 | Launcher argument shape with no map: `--mod <m> --app-name <name>` | `resolve_map_path` scan steps over the `--app-name` value | No boot map; frontend shown. `--app-name <name> maps/x.prl` loads `maps/x.prl` |
| P5 | `--app-name` bare, `--app-name=`, or `--app-name` followed by another flag | stage 1 | Boot error naming the flag; no fallback to `postretro` |
| P6 | SDK bundle assembled by `sdk-dist` through the shared `emit_launcher` | launcher emission | Bundle marker names the package `<package>-sdk`; emitted launcher carries `--app-name <package>-sdk`; a bundle launch never writes under the game's directory |
