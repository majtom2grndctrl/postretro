import { enemies } from "postretro";

const release = enemies({ tag: "closet_a" }).update({ aggro: true });
const hold = enemies({ tag: "closet_a" }).update({ aggro: false });
const emptyPartialIsValid = enemies({ tag: "closet_a" }).update({});

// The selector accepts an omitted tag so raw descriptor construction and the
// handle share the bind-time rejection contract; the engine rejects it then.
const bindTimeRejected = enemies({}).update({ aggro: true });

// @ts-expect-error `aggro` is a boolean enemy-state field.
enemies({ tag: "closet_a" }).update({ aggro: "open" });
// @ts-expect-error Enemy-state updates are a closed, typed partial.
enemies({ tag: "closet_a" }).update({ aggression: true });

void release;
void hold;
void emptyPartialIsValid;
void bindTimeRejected;
