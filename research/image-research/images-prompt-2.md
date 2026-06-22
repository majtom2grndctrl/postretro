  - id: glass_atrium_sky_reflective_01
    filename: skymall_glass_atrium_sky_reflective_01
    size: 64x64
    tileable: true
    maps: [diffuse]
    alpha: true
    trenchbroom_use: Skylight panels, exterior atrium windows.
    prompt: >
      Create a seamless 64x64 pixel stylized blue atrium glass texture for a
      late-1990s skymall. Cyan-blue tint, chunky white cloud reflections, broad
      sky-blue albedo bands, dark frame-adjacent color bands, low-res
      hand-painted pixels. Diffuse albedo should use flat lighting with no
      painted shadows, highlights, ambient occlusion, contact darkening, or
      directional light. Output one individual texture PNG, not a sheet or
      atlas.
    negative_prompt: >
      No photoreal glass, no raytraced reflections, no tiny window dirt, no
      modern transparent shader complexity, no baked lighting.
    acceptance_criteria:
      - Reads as bright sky-reflective glass.
      - Works in repeated skylight panels.
      - Reflection shapes are chunky and abstract.

  - id: glass_railing_cyan_smoke_01
    filename: skymall_glass_railing_cyan_smoke_01
    size: 64x64
    tileable: true
    maps: [diffuse]
    alpha: true
    trenchbroom_use: Balcony railings, bridge railings, shop barriers.
    prompt: >
      Create a seamless 64x64 pixel cyan smoky glass railing texture. Dark teal
      transparent glass, chunky horizontal cyan edge color bands along top and
      bottom, subtle blocky albedo bands, retro low-poly mall style. Diffuse
      albedo should use flat lighting with no painted shadows, highlights,
      ambient occlusion, contact darkening, or directional light. Output one
      individual texture PNG, not a sheet or atlas.
    negative_prompt: >
      No realistic fingerprints, no dense scratches, no opaque wall material,
      no modern PBR glass, no baked lighting.
    acceptance_criteria:
      - Distinct from bright skylight glass.
      - Usable as railing panels.
      - Strong readable cyan edge color bands.

  - id: metal_dark_blue_trim_01
    filename: skymall_metal_dark_blue_trim_01
    size: 64x64
    tileable: true
    maps: [diffuse, specular, normal]
    trenchbroom_use: Rail caps, storefront trim, kiosk frames, sign mounts.
    prompt: >
      Create a seamless 64x64 pixel dark blue-gray metal trim texture for a
      retro-futurist skymall. Muted charcoal teal, broad hand-painted edge
      color bands, subtle panel seams, low-res brushed metal impression.
      Diffuse albedo should use flat lighting with no painted shadows,
      highlights, ambient occlusion, contact darkening, or directional light.
      Output one individual texture PNG, not a sheet or atlas.
    negative_prompt: >
      No shiny chrome, no rusty industrial metal, no photoreal scratches, no
      complex procedural anisotropy, no baked lighting.
    acceptance_criteria:
      - Reads as durable public-infrastructure metal.
      - Works on thin trim brushes.
      - Low-res material bands are clear.

  - id: metal_pale_skylight_truss_01
    filename: skymall_metal_pale_skylight_truss_01
    size: 64x64
    tileable: true
    maps: [diffuse, specular, normal]
    trenchbroom_use: Skylight beams, roof lattice, structural braces.
    prompt: >
      Create a seamless 64x64 pixel pale painted aluminum truss texture for a
      futuristic skymall skylight. Warm off-white metal, blocky edge wear,
      simple bolt hints, clean atrium structure, late-90s hand-painted texture.
      Diffuse albedo should use flat lighting with no painted shadows,
      highlights, ambient occlusion, contact darkening, or directional light.
      Output one individual texture PNG, not a sheet or atlas.
    negative_prompt: >
      No black industrial steel, no rust, no photoreal bolts, no spaceship hull,
      no baked lighting.
    acceptance_criteria:
      - Feels bright and civic.
      - Suitable for diagonal roof beam brushes.
      - Edge color bands help simple geometry read.

  - id: trim_concrete_cap_01
    filename: skymall_trim_concrete_cap_01
    size: 64x16
    tileable: true
    maps: [diffuse, specular, normal]
    trenchbroom_use: Planter lips, balcony edges, stair caps, wall caps.
    prompt: >
      Create a seamless 64x16 pixel concrete cap trim for a retro skymall.
      Pale beige-gray stone, distinct top and lower material bands, chunky
      chips, readable cap shape without painted lighting. Diffuse albedo should
      use flat lighting with no painted shadows, highlights, ambient occlusion,
      contact darkening, or directional light. Output one individual texture
      PNG, not a sheet or atlas.
    negative_prompt: >
      No photoreal bevel, no ornate molding, no medieval stone trim, no baked
      lighting.
    acceptance_criteria:
      - Clear top/bottom material bands.
      - Tiles horizontally.
      - Useful as a finishing strip on brush edges.

  - id: trim_cyan_light_strip_01
    filename: skymall_trim_cyan_light_strip_01
    size: 64x16
    tileable: true
    maps: [diffuse]
    trenchbroom_use: Sign borders, kiosk seams, bridge accents, ceiling strips.
    prompt: >
      Create a seamless 64x16 pixel cyan sci-fi light strip texture for a
      late-90s skymall. Bright cyan albedo center line, darker teal border
      pixels, black-blue casing edge, flat luminous-panel color without bloom
      or light spill. Diffuse albedo should use flat lighting with no painted
      shadows, highlights, ambient occlusion, contact darkening, or directional
      light. Output one individual texture PNG, not a sheet or atlas.
    negative_prompt: >
      No neon cyberpunk overload, no photoreal bloom, no complex LED array, no
      emissive map output.
    acceptance_criteria:
      - Reads clearly as a bright sci-fi light-strip material.
      - Loops horizontally.
      - Works as subtle accent, not dominant lighting.

  - id: light_square_ceiling_panel_01
    filename: skymall_light_square_ceiling_panel_01
    size: 32x32
    tileable: false
    maps: [diffuse]
    trenchbroom_use: Recessed ceiling lights under balconies.
    prompt: >
      Create a 32x32 pixel square recessed ceiling light for a retro futuristic
      skymall. Pale cyan-white luminous panel color, dark rim, simple flat lens
      pixels, designed for placement on dark soffit concrete. Diffuse albedo
      should use flat lighting with no painted shadows, highlights, ambient
      occlusion, contact darkening, or directional light. Output one individual
      texture PNG, not a sheet or atlas.
    negative_prompt: >
      No realistic fixture detail, no lens flare, no ornate lamp, no bloom halo,
      no emissive map output.
    acceptance_criteria:
      - Works as a small repeated ceiling light.
      - Has a visible dark frame.
      - Reads at low resolution.
