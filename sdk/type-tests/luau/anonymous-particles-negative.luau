--!strict
-- Expected: two TypeError diagnostics; particle_state and particle are outside SDK.ComponentKind.
local SDK = require("./component-kind")
local removedKind: SDK.ComponentKind = "particle_state"
local removedAlias: SDK.ComponentKind = "particle"
return { removedKind, removedAlias }
