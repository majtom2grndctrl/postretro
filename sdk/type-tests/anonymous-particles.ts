import type { ComponentKind, ComponentValue } from "postretro";

const emitter: ComponentKind = "billboard_emitter";
// @ts-expect-error Anonymous pool particles have no component kind.
const particleState: ComponentKind = "particle_state";
// @ts-expect-error The historical FFI alias is also removed.
const particleAlias: ComponentKind = "particle";
// @ts-expect-error Anonymous pool particles have no component value.
const particleValue: ComponentValue = { kind: "particle_state" };
// @ts-expect-error The removed state type is not exported.
import type { ParticleState } from "postretro";

void [emitter, particleState, particleAlias, particleValue];
