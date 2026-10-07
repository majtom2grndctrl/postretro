--!nocheck
-- Target/command builders over subject tokens and tags: damage, resource
-- grants, per-owner slot deltas, live enemy and spawner groups, and
-- fired-trigger arm/disarm.
-- Part chunk of `data_script.luau`: evaluated after `data_script/reactions.luau`
-- (whose opaque target tokens it compares by identity), published through the
-- temporary `__postretroDataScriptParts` bridge, and merged into its returned
-- builder table. Type names refer to the declarations in `data_script.luau`.
-- See: context/lib/scripting.md §7 (SDK library globals), §12

local reactions = __postretroDataScriptParts.reactions
local ACTIVATORS_TARGET = reactions.ACTIVATORS_TARGET
local TRIGGER_TARGET = reactions.TRIGGER_TARGET

local CommandsSdk = {}

function CommandsSdk.damage(target: ActivatorsTarget | string, amount: number): PrimitiveReactionDescriptor
  if type(target) == "string" then
    return { primitive = "applyDamage", tag = target, args = { amount = amount } }
  end
  local wireTarget: any = if target == ACTIVATORS_TARGET then "@activators" else "@invalid"
  return { primitive = "applyDamage", target = wireTarget, args = { amount = amount } }
end

--- Grant health to the current trigger activators or every entity with a tag.
function CommandsSdk.grantHealth(target: ActivatorsTarget | string, amount: number): PrimitiveReactionDescriptor
  if type(target) == "string" then
    return { primitive = "grantHealth", tag = target, args = { amount = amount } }
  end
  local wireTarget: any = if target == ACTIVATORS_TARGET then "@activators" else "@invalid"
  return { primitive = "grantHealth", target = wireTarget, args = { amount = amount } }
end

--- Grant an ammo-reserve pool to the current trigger activators or every entity with a tag.
function CommandsSdk.grantAmmo(target: ActivatorsTarget | string, ammoType: string, amount: number): PrimitiveReactionDescriptor
  if type(target) == "string" then
    return { primitive = "grantAmmo", tag = target, args = { type = ammoType, amount = amount } }
  end
  local wireTarget: any = if target == ACTIVATORS_TARGET then "@activators" else "@invalid"
  return { primitive = "grantAmmo", target = wireTarget, args = { type = ammoType, amount = amount } }
end

--- Add a delta to a per-owner numeric slot for current trigger activators or
--- every player pawn with a tag.
function CommandsSdk.addSlot(target: ActivatorsTarget | string, slot: StateRef<number>, delta: number): PrimitiveReactionDescriptor
  if type(target) == "string" then
    return { primitive = "addSlot", tag = target, args = { slot = slot.slot, delta = delta } }
  end
  local wireTarget: any = if target == ACTIVATORS_TARGET then "@activators" else "@invalid"
  return { primitive = "addSlot", target = wireTarget, args = { slot = slot.slot, delta = delta } }
end

--- Selects a live enemy group addressed when a reaction fires.
export type EnemyGroupFilter = { tag: string? }

--- Typed, additive partial for consequential enemy-state updates.
export type EnemyStateUpdateArgs = { aggro: boolean? }

--- Fire-time-tag enemy handle. `update` emits one primitive descriptor.
export type EnemyGroup = {
  update: (self: EnemyGroup, fields: EnemyStateUpdateArgs) -> PrimitiveReactionDescriptor,
}

--- Select a live enemy group by tag. The tag resolves when the reaction fires.
function CommandsSdk.enemies(filter: EnemyGroupFilter): EnemyGroup
  return {
    update = function(_self: EnemyGroup, fields: EnemyStateUpdateArgs): PrimitiveReactionDescriptor
      return { primitive = "updateEnemyState", tag = filter.tag, args = fields }
    end,
  }
end

--- Selects the live spawner group addressed when a reaction fires.
export type SpawnerFilter = { tag: string }

--- Fire-time-tag spawner handle. `fire` emits one primitive descriptor.
export type SpawnerHandle = {
  fire: (self: SpawnerHandle) -> PrimitiveReactionDescriptor,
}

--- Select a live spawner group by tag and fire each matching spawner.
function CommandsSdk.spawner(filter: SpawnerFilter): SpawnerHandle
  return {
    fire = function(_self: SpawnerHandle): PrimitiveReactionDescriptor
      return { primitive = "spawnFromSpawner", tag = filter.tag }
    end,
  }
end

function CommandsSdk.armTrigger(target: TriggerTarget): {SequenceStep}
  local wireTarget: any = if target == TRIGGER_TARGET then "@trigger" else "@invalid"
  return { { id = wireTarget, primitive = "armTrigger", args = {} } :: any }
end

function CommandsSdk.disarmTrigger(target: TriggerTarget): {SequenceStep}
  local wireTarget: any = if target == TRIGGER_TARGET then "@trigger" else "@invalid"
  return { { id = wireTarget, primitive = "disarmTrigger", args = {} } :: any }
end

return {
  builders = CommandsSdk,
}
