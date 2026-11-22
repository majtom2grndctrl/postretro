import { defineMod, type RenderProfile } from "postretro";

const identity = { name: "render-profile", id: "render.profile", version: "1" };

// Every field, inline in `defineMod`, where authors write it.
const full = defineMod({
  ...identity,
  render: {
    bloom: { resolution: "eighth", pixelated: true },
    reflections: { style: "smooth" },
    specular: { style: "smooth" },
    ghosts: "dither",
  },
});
const empty = defineMod({ ...identity, render: {} });

const badSpecular = defineMod({
  ...identity,
  // @ts-expect-error Specular style is `pixel` or `smooth`.
  render: { specular: { style: "sharp" } },
});
const badReflection = defineMod({
  ...identity,
  // @ts-expect-error Reflection style is `pixel` or `smooth`.
  render: { reflections: { style: "mirror" } },
});
const badBloom = defineMod({
  ...identity,
  // @ts-expect-error Bloom resolution has no `full` step.
  render: { bloom: { resolution: "full" } },
});
const badGhosts = defineMod({
  ...identity,
  // @ts-expect-error Ghost transparency is `blend` or `dither`.
  render: { ghosts: "fade" },
});
const misspelled = defineMod({
  ...identity,
  // @ts-expect-error The render node is closed: a misspelled field is an excess property.
  render: { speculars: { style: "pixel" } },
});

// @ts-expect-error `pixelated` is a boolean, not a style name.
const pixelatedString: RenderProfile = { bloom: { pixelated: "pixel" } };

export { full, empty, badSpecular, badReflection, badBloom, badGhosts, misspelled, pixelatedString };
