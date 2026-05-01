--!strict
-- Transform-only entity vocabulary: lightweight handle exposed by
-- `world:query({ component = "transform" })`. Same shape as `EntityHandle`;
-- declared here for symmetry with `entities/lights.luau` and
-- `entities/emitters.luau`.

--- Handle returned by `world:query({ component = "transform" })`.
export type TransformHandle = {
  id: EntityId,
  position: Vec3,
  tags: {string},
}

return {}
