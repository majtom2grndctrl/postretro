--!strict
-- Private fluent-expression implementation shared by descriptor builders.
-- The prelude supplies this table temporarily; consumers capture its functions.

local numberNodes: {[any]: any} = {}
local boolNodes: {[any]: any} = {}
local boolRef: (node: any) -> BoolRef

local function constant(value: number | boolean): any
  return { op = "const", value = value }
end

local function numberNode(value: NumberValue): any
  if type(value) == "number" then
    return constant(value)
  end
  return numberNodes[value] or value
end

local function boolNode(value: BoolValue): any
  if type(value) == "boolean" then
    return constant(value)
  end
  return boolNodes[value] or value
end

local function numberRef(node: any): NumberRef
  local ref: any = {}
  ref.plus = function(_self: NumberRef, n: NumberValue): NumberRef
    return numberRef({ op = "add", a = node, b = numberNode(n) })
  end
  ref.minus = function(_self: NumberRef, n: NumberValue): NumberRef
    return numberRef({ op = "sub", a = node, b = numberNode(n) })
  end
  ref.times = function(_self: NumberRef, n: NumberValue): NumberRef
    return numberRef({ op = "mul", a = node, b = numberNode(n) })
  end
  ref.dividedBy = function(_self: NumberRef, n: NumberValue): NumberRef
    return numberRef({ op = "div", a = node, b = numberNode(n) })
  end
  ref.clamp = function(_self: NumberRef, lo: NumberValue, hi: NumberValue): NumberRef
    return numberRef({ op = "clamp", x = node, lo = numberNode(lo), hi = numberNode(hi) })
  end
  ref.lerp = function(_self: NumberRef, to: NumberValue, t: NumberValue): NumberRef
    return numberRef({ op = "lerp", a = node, b = numberNode(to), t = numberNode(t) })
  end
  ref.lt = function(_self: NumberRef, n: NumberValue): BoolRef
    return boolRef({ op = "lt", a = node, b = numberNode(n) })
  end
  ref.le = function(_self: NumberRef, n: NumberValue): BoolRef
    return boolRef({ op = "le", a = node, b = numberNode(n) })
  end
  ref.gt = function(_self: NumberRef, n: NumberValue): BoolRef
    return boolRef({ op = "gt", a = node, b = numberNode(n) })
  end
  ref.ge = function(_self: NumberRef, n: NumberValue): BoolRef
    return boolRef({ op = "ge", a = node, b = numberNode(n) })
  end
  ref.eq = function(_self: NumberRef, n: NumberValue): BoolRef
    return boolRef({ op = "eq", a = node, b = numberNode(n) })
  end
  ref.ne = function(_self: NumberRef, n: NumberValue): BoolRef
    return boolRef({ op = "ne", a = node, b = numberNode(n) })
  end
  numberNodes[ref] = node
  return table.freeze(ref) :: any
end

boolRef = function(node: any): BoolRef
  local ref: any = {}
  ref["and"] = function(_self: BoolRef, other: BoolValue): BoolRef
    return boolRef({ op = "select", cond = node, a = boolNode(other), b = constant(false) })
  end
  ref["or"] = function(_self: BoolRef, other: BoolValue): BoolRef
    return boolRef({ op = "select", cond = node, a = constant(true), b = boolNode(other) })
  end
  ref["not"] = function(_self: BoolRef): BoolRef
    return boolRef({ op = "select", cond = node, a = constant(false), b = constant(true) })
  end
  ref.select = function(_self: BoolRef, whenTrue: NumberValue, whenFalse: NumberValue): NumberRef
    return numberRef({ op = "select", cond = node, a = numberNode(whenTrue), b = numberNode(whenFalse) })
  end
  boolNodes[ref] = node
  return table.freeze(ref) :: any
end

return table.freeze({
  numberNode = numberNode,
  boolNode = boolNode,
  numberRef = numberRef,
  boolRef = boolRef,
})
