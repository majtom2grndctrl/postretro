# Indirect validation screenshots

Seven views, fourteen 1280×720 PNGs. Every validation-on/default-off pair is byte-identical. Captured from the same symbol-preserving release binary with `capture`, without `dev-tools`, on AMD Radeon Pro 5300M / Metal.

These static captures supplement M4. They run no script VM, gameplay tick, HUD, or simulated walk. M1 foreground timings, M2 profiles, M3 full startup matrix, and M4 owner/live-walk verdict remain outstanding. Physical display power state was not inspected.

[Open the side-by-side gallery](gallery.html). PNGs and logs stay local; scene inputs and comparison hashes are versioned.

| View | Validation on | Release default off | Result |
|---|---|---|---|
| Campaign — spawn | [PNG](campaign-spawn-on.png) | [PNG](campaign-spawn-off.png) | byte-identical |
| Campaign — nearby view | [PNG](campaign-nearby-on.png) | [PNG](campaign-nearby-off.png) | byte-identical |
| Hallway — spawn | [PNG](hallway-spawn-on.png) | [PNG](hallway-spawn-off.png) | byte-identical |
| Hallway — nearby view | [PNG](hallway-nearby-on.png) | [PNG](hallway-nearby-off.png) | byte-identical |
| Hallway — turn 90° | [PNG](hallway-corridor-on.png) | [PNG](hallway-corridor-off.png) | byte-identical |
| Hallway — turn 180° | [PNG](hallway-reverse-on.png) | [PNG](hallway-reverse-off.png) | byte-identical |
| Hallway — side passage | [PNG](hallway-side-on.png) | [PNG](hallway-side-off.png) | byte-identical |

Reproduce from the workspace root:

```sh
CARGO_PROFILE_RELEASE_STRIP=none cargo build --release -p postretro --features capture
RUST_LOG=info POSTRETRO_SH_STREAMING=sync-proof WGPU_VALIDATION_INDIRECT_CALL=1 target/release/postretro --capture measurements/release-indirect-validation/screenshots/campaign-spawn-on.scene.json
env -u WGPU_VALIDATION_INDIRECT_CALL RUST_LOG=info POSTRETRO_SH_STREAMING=sync-proof target/release/postretro --capture measurements/release-indirect-validation/screenshots/campaign-spawn-off.scene.json
```

Repeat with each paired scene file in this directory. `comparison.json` pins the build hash, map hashes, camera settings, and PNG hashes.
