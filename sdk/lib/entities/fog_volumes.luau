--!strict
-- FogVolumeHandle: type alias for the query-time FogVolumeEntity snapshot.
-- No engine-side fog animation primitive exists (unlike lights), and the
-- Live VM script tick API has been removed, so the handle exposes only
-- the snapshot fields until a declarative fog-animation primitive lands.
-- Fog ambient color is derived from the SH irradiance volume and is not
-- settable via script.
-- See docs/scripting-reference.md.

-- `FogVolumeComponent` and `FogVolumeEntity` are provided ambiently by
-- the generated `sdk/types/postretro.d.luau` definitions file.

--- Typed handle returned by `world:query` for a fog-volume entity.
export type FogVolumeHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
  component: FogVolumeComponent,
}

local FogVolumesSdk = {}

--- Wrap a `FogVolumeEntity` snapshot returned by `worldQuery`. Used by
--- `world.luau` for `world:query({ component = "fog_volume" })`. With
--- the tick-callback helpers removed, this is a pass-through; it is
--- retained so the world.query code path stays symmetric with
--- `wrapLightEntity` and so a future fog-animation primitive can
--- re-introduce mutating methods without touching the call site.
function FogVolumesSdk.wrapFogVolumeEntity(snapshot: FogVolumeEntity): FogVolumeHandle
  return {
    id = snapshot.id,
    position = snapshot.position,
    tags = snapshot.tags,
    component = snapshot.component,
  }
end

return FogVolumesSdk
