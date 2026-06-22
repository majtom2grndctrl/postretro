  - id: foliage_shrub_dense_01
    filename: skymall_foliage_shrub_dense_01
    size: 64x64
    tileable: true
    alpha: true
    maps: [diffuse]
    trenchbroom_use: Planter filler, low shrub alpha cards.
    prompt: >
      Create a 64x64 pixel alpha texture of dense curated indoor shrubbery for
      a bright skymall. Chunky clusters of dark green, moss green, yellow-green,
      occasional tiny red flower pixels, maintained public-atrium landscaping.
      Diffuse albedo should use flat lighting with no painted shadows,
      highlights, ambient occlusion, contact darkening, or directional light.
      Output one individual texture PNG with alpha, not a sheet or atlas.
    negative_prompt: >
      No photoreal leaves, no abandoned overgrowth, no jungle chaos, no fine
      botanical detail, no baked lighting.
    acceptance_criteria:
      - Reads as dense maintained planter greenery.
      - Alpha edges are usable on simple foliage planes.
      - Pixel clusters remain chunky.

  - id: foliage_hanging_vines_01
    filename: skymall_foliage_hanging_vines_01
    size: 64x128
    tileable: false
    alpha: true
    maps: [diffuse]
    trenchbroom_use: Vines hanging over balcony ledges and planter walls.
    prompt: >
      Create a 64x128 pixel alpha texture of hanging balcony vines for a
      retro-futurist skymall. Vertical trailing leaves, curated indoor
      greenery, dark and light green chunky pixel clusters, transparent
      background, designed for ledge-hanging planes. Diffuse albedo should use
      flat lighting with no painted shadows, highlights, ambient occlusion,
      contact darkening, or directional light. Output one individual texture
      PNG with alpha, not a sheet or atlas.
    negative_prompt: >
      No wild abandoned overgrowth, no photoreal ivy, no thin detailed stems,
      no horror decay, no baked lighting.
    acceptance_criteria:
      - Works as a vertical alpha card.
      - Looks maintained and intentional.
      - Helps soften concrete balcony edges.

  - id: foliage_broadleaf_potted_01
    filename: skymall_foliage_broadleaf_potted_01
    size: 64x128
    tileable: false
    alpha: true
    maps: [diffuse]
    trenchbroom_use: Indoor potted plants and planter feature cards.
    prompt: >
      Create a 64x128 pixel alpha texture of a broadleaf indoor architectural
      plant for a skymall. Large simple leaf silhouettes, dark green and
      yellow-green leaf color pixels, chunky hand-painted low-res pixels,
      curated public-space landscaping. Diffuse albedo should use flat lighting
      with no painted shadows, highlights, ambient occlusion, contact
      darkening, or directional light. Output one individual texture PNG with
      alpha, not a sheet or atlas.
    negative_prompt: >
      No photoreal plant, no tiny leaf detail, no jungle tree, no dead plant,
      no baked lighting.
    acceptance_criteria:
      - Clear broadleaf silhouette.
      - Works on crossed planes or flat cards.
      - Style matches shrub and vine textures.

  - id: planter_soil_mulch_01
    filename: skymall_planter_soil_mulch_01
    size: 64x64
    tileable: true
    maps: [diffuse, specular, normal]
    trenchbroom_use: Planter beds under foliage.
    prompt: >
      Create a seamless 64x64 pixel dark soil and mulch texture for maintained
      indoor skymall planters. Dark brown-black base, chunky bark flecks,
      occasional green pixels, clean public landscaping, retro game texture.
      Diffuse albedo should use flat lighting with no painted shadows,
      highlights, ambient occlusion, contact darkening, or directional light.
      Output one individual texture PNG, not a sheet or atlas.
    negative_prompt: >
      No mud, no decomposing trash, no photoreal soil scan, no tiny noise, no
      baked lighting.
    acceptance_criteria:
      - Reads as planter mulch.
      - Loops cleanly.
      - Does not dominate foliage visually.
