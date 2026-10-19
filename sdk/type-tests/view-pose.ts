import type { LedgeFocus, PlayerMovementDescriptor, PoseParams, PoseRow, VaultPoseRow, ViewFeelParams } from "postretro";

// The Scripting surface example: a vault row with ledge focus and literal holds, and a rate-timed slide row.
const example = {
  pose: {
    vault: {
      ledgeFocus: { strength: 0.7, maxPitch: 30 },
      roll: 10, fov: -30,
      easeIn: 0.33, easeOut: 0.33,
    },
    slide: { eyeDrop: 0.15, fov: 5, enterRate: 18, exitRate: 12 },
  },
} satisfies ViewFeelParams;
// Every row and channel is optional; the vault row needs no field at all.
const sparse = { vault: {} } satisfies PoseParams;
const noPose = {} satisfies ViewFeelParams;
const movementView: Pick<PlayerMovementDescriptor, "viewFeel"> = { viewFeel: example };

// @ts-expect-error A rate row needs both rates.
const missingRate: PoseRow = { eyeDrop: 0.15, enterRate: 18 };
// @ts-expect-error The vault row is timed by shares, not rates.
const vaultRate: VaultPoseRow = { pitch: -10, enterRate: 18 };
// @ts-expect-error The slide row is timed by rates, not shares.
const slideShare: PoseRow = { enterRate: 18, exitRate: 12, easeIn: 0.2 };
// @ts-expect-error Ledge focus is vault-only.
const slideFocus: PoseParams = { slide: { enterRate: 18, exitRate: 12, ledgeFocus: { strength: 0.5, maxPitch: 30 } } };
// @ts-expect-error `normal` has no row: a pose there would be a permanent offset.
const normalRow: PoseParams = { normal: { pitch: -5 } };
// @ts-expect-error Unknown row field.
const typo: VaultPoseRow = { pich: -10 };
// @ts-expect-error `fovIncrease` is now `fov`.
const renamed: PoseRow = { fovIncrease: 5, enterRate: 18, exitRate: 12 };
// @ts-expect-error Unknown ledge focus field.
const focusTypo: LedgeFocus = { strength: 0.5, maxPitch: 30, max: 20 };
// @ts-expect-error Ledge focus needs both fields.
const partialFocus: VaultPoseRow = { ledgeFocus: { strength: 0.7 } };
// @ts-expect-error Channels are numbers.
const wordPitch: VaultPoseRow = { pitch: "down" };
// @ts-expect-error The slide block moved to `pose.slide`.
const leftoverSlide: ViewFeelParams = { slide: { eyeDrop: 0.15, enterRate: 18, exitRate: 12 } };
// @ts-expect-error The vault block moved to `pose.vault`.
const leftoverVault: ViewFeelParams = { vault: { pitch: -38.5 } };
void [sparse, noPose, movementView, missingRate, vaultRate, slideShare, slideFocus, normalRow, typo, renamed, focusTypo, partialFocus, wordPitch, leftoverSlide, leftoverVault];
