// Reference durable mod state: persisted locally and replicated by the host.

import { defineStore } from "postretro";

export const runCounter = defineStore({
  count: {
    type: "number",
    default: 0,
    persist: true,
    network: "shared",
  },
});
