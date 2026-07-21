// Compatibility import path for E16 fixtures. The authoring surface now lives
// in the shipped `postretro` SDK; retain this module so existing fixtures can
// demonstrate the staged migration without shadowing its public types.
declare module "postretro/proposed" {
  export * from "postretro";
}
