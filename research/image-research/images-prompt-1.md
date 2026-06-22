textures:
  - id: floor_warm_stone_tile_square_01
    filename: skymall_floor_warm_stone_tile_square_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; boundary grout forms one normal seam
    category: floor
    priority: essential
    intended_use: Main atrium floor, large public circulation areas.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of warm beige square stone
      mall flooring for a late-1990s low-poly 3D game skymall. Use large civic
      plaza tiles, chunky dark grout, cream/tan/warm-gray variation, subtle
      hand-painted chips, clean public-atrium polish, broad pixel clusters,
      readable tile divisions, no photorealism.
    acceptance:
      - Reads as clean public mall flooring.
      - Strong square tile grid.
      - Loops cleanly in both directions.
      - No tiny speckled noise.

  - id: floor_warm_stone_tile_square_02_alt
    filename: skymall_floor_warm_stone_tile_square_02_alt.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; boundary grout forms one normal seam
    category: floor
    priority: high
    intended_use: Alternate main floor variation to reduce repetition.
    prompt: >
      Create a seamless 64x64 pixel alternate warm stone mall floor texture for
      a retro futuristic skymall. Similar to civic beige limestone tiles, but
      with slightly more cream and pale ochre variation, chunky pixel grout,
      a few broad scuffed areas, and hand-painted low-res public-space wear.
    acceptance:
      - Compatible with floor_warm_stone_tile_square_01.
      - Different enough to break repetition.
      - Still bright, clean, and civic.

  - id: floor_rectangular_concourse_tile_01
    filename: skymall_floor_rectangular_concourse_tile_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; boundary rows read like normal interior rows
    category: floor
    priority: essential
    intended_use: Upper concourses, secondary walkways, bridge decks.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of rectangular concourse
      tiles for a bright futuristic skymall. Use cool beige, pale gray, and
      muted blue-gray rectangular slabs with chunky grout lines. It should feel
      like a public transit/mall walkway in a late-1990s low-poly game.
    acceptance:
      - Rectangular pattern is distinct from main square floor.
      - Cool enough for upper walkways.
      - Seamless and brush-friendly.

  - id: floor_bridge_bluegray_panel_01
    filename: skymall_floor_bridge_bluegray_panel_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel seams do not double at boundaries
    category: floor
    priority: high
    intended_use: Skybridge walking surfaces, elevated decks.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of blue-gray composite
      bridge deck panels for a retro futuristic skymall. Use broad rectangular
      panels, subtle seams, muted teal-gray tones, light scuffs, and clean
      public-infrastructure wear. Chunky late-90s hand-painted pixel style.
    acceptance:
      - Reads as elevated bridge/deck flooring.
      - Cooler and more technical than atrium stone.
      - Works on long narrow bridge brushes.

  - id: floor_shop_threshold_tile_01
    filename: skymall_floor_shop_threshold_tile_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; boundary grout forms one normal seam
    category: floor
    priority: medium
    intended_use: Store entrances, shop thresholds, transition strips.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of polished shop threshold
      tile for a retro skymall. Slightly darker beige and warm gray stone,
      smaller tile divisions, clean commercial entry feel, chunky pixel grout,
      subtle hand-painted shine, no modern photoreal gloss.
    acceptance:
      - Reads as a transition between mall floor and shop.
      - Slightly more polished than main floor.
      - Tileable and low-res.

  - id: floor_service_gray_tile_01
    filename: skymall_floor_service_gray_tile_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; boundary grout forms one normal seam
    category: floor
    priority: medium
    intended_use: Back corridors, utility corners, maintenance-adjacent zones.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of muted gray public-service
      floor tile for a skymall. Simple square gray-blue tiles, darker grout,
      mild scuffs, clean but less premium than the atrium floor, late-1990s
      chunky pixel texture style.
    acceptance:
      - Useful for secondary spaces.
      - Less warm than public atrium tile.
      - Not grimy or industrial.

  - id: floor_atrium_mosaic_round_01
    filename: skymall_floor_atrium_mosaic_round_01.png
    size: 128x128
    tileable: false
    tiling:
      mode: non_repeating
      edge_contract: no edge matching required
      edge_phase: none
      qa_preview: inspect as a single centered panel
    category: floor_hero
    priority: essential
    intended_use: Central atrium medallion on a square floor brush.
    prompt: >
      Create a 128x128 pixel non-tileable decorative atrium floor medallion for
      a late-1990s low-poly skymall. Centered circular or octagonal mosaic,
      blue-green sky/water motif, cream and tan stone border, dark chunky grout,
      radial civic design, low-resolution hand-painted pixel art.
    acceptance:
      - Centered composition.
      - Looks like inlaid mall/civic mosaic.
      - No fantasy rune or ornate mandala language.

  - id: floor_atrium_mosaic_corner_01
    filename: skymall_floor_atrium_mosaic_corner_01.png
    size: 64x64
    tileable: false
    tiling:
      mode: non_repeating
      edge_contract: no edge matching required
      edge_phase: none
      qa_preview: inspect as a single accent face
    category: floor_hero
    priority: medium
    intended_use: Corner/edge pieces around a larger mosaic zone.
    prompt: >
      Create a 64x64 pixel decorative mosaic corner floor texture for a retro
      skymall atrium. Blue-green and cream inlaid stone pieces, chunky dark
      grout, partial curved border passing through the tile, designed to
      surround a larger central floor medallion.
    acceptance:
      - Clearly reads as a partial medallion/corner piece.
      - Compatible with the round atrium mosaic.
      - Usable as accent floor brush face.

  - id: wall_pale_civic_concrete_plain_01
    filename: skymall_wall_pale_civic_concrete_plain_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: none
      qa_preview: inspect as a 2x2 repeat; mottling has no obvious boundary
    category: wall
    priority: essential
    intended_use: Large walls, columns, atrium structural surfaces.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of pale civic concrete /
      limestone for a bright futuristic skymall. Warm beige-gray base, blocky
      mottling, subtle stains, slight chipped pixels, clean premium public
      architecture, hand-painted late-1990s game texture.
    acceptance:
      - Clean, bright, and civic.
      - Not dungeon stone.
      - Not brutalist or industrial.
      - Loops cleanly.

  - id: wall_pale_civic_concrete_plain_02_alt
    filename: skymall_wall_pale_civic_concrete_plain_02_alt.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: none
      qa_preview: inspect as a 2x2 repeat; mottling has no obvious boundary
    category: wall
    priority: high
    intended_use: Alternate wall material for repetition control.
    prompt: >
      Create a seamless 64x64 pixel alternate pale civic concrete texture for a
      retro skymall. Slightly warmer limestone tone, broader hand-painted
      patches, a few soft vertical stains, small blocky chips, clean public
      atrium character.
    acceptance:
      - Compatible with plain_01.
      - Has visible variation without looking dirty.
      - Suitable for broad brush walls.

  - id: wall_large_concrete_panel_grid_01
    filename: skymall_wall_large_concrete_panel_grid_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel seams do not double at boundaries
    category: wall_panel
    priority: essential
    intended_use: Modular wall panels, balcony faces, planter sides.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of large pale concrete wall
      panels for a futuristic skymall. Rectangular panel divisions, warm
      gray-beige concrete, chunky seam lines, subtle edge wear, clean civic
      public-building feel, late-90s pixel texture.
    acceptance:
      - Panel seams are obvious and grid-friendly.
      - Useful on modular brush architecture.
      - Reads as clean public construction.

  - id: wall_horizontal_limestone_bands_01
    filename: skymall_wall_horizontal_limestone_bands_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; horizontal bands keep normal row weight
    category: wall_panel
    priority: medium
    intended_use: Long hallway walls, balcony bands, retaining walls.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of horizontal limestone /
      pale concrete bands for a retro futuristic skymall. Broad stacked bands,
      warm beige-gray stone, darker horizontal seams, chunky hand-painted
      mottling, clean public architecture.
    acceptance:
      - Strong horizontal structure.
      - Useful for long wall runs.
      - Does not look like brick or dungeon blocks.

  - id: wall_vertical_column_concrete_01
    filename: skymall_wall_vertical_column_concrete_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_v
      edge_contract: top/bottom edges must match; left/right are bounded column-face edges
      edge_phase: none
      qa_preview: inspect as a 1x3 vertical repeat
    category: column
    priority: high
    intended_use: Pillars, square columns, vertical supports.
    prompt: >
      Create a seamless 64x64 pixel vertical pale concrete column texture for a
      bright skymall. Subtle vertical gradient, warm limestone-gray mottling,
      simple edge-darkening hints, small blocky chips, clean structural support
      material in late-90s game style.
    acceptance:
      - Vertical directionality is clear.
      - Works on square brush columns.
      - Feels like atrium support, not classical column.

  - id: wall_column_corner_shadow_01
    filename: skymall_wall_column_corner_shadow_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_v
      edge_contract: top/bottom edges must match; left/right are bounded column-side edges
      edge_phase: none
      qa_preview: inspect as a 1x3 vertical repeat
    category: column
    priority: medium
    intended_use: Column side faces and corner-shadow variation.
    prompt: >
      Create a seamless 64x64 pixel pale concrete column-side texture for a
      retro skymall, with a subtle darker vertical edge shadow on one side,
      warm beige-gray surface, chunky mottling, and clean public architecture.
    acceptance:
      - Useful for giving brush columns depth.
      - One side has visible edge shadow.
      - Still tileable vertically.

  - id: wall_planter_concrete_side_01
    filename: skymall_wall_planter_concrete_side_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel seams do not double at boundaries
    category: planter_architecture
    priority: high
    intended_use: Raised planter side walls.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture for pale concrete planter
      side walls in a futuristic skymall. Warm civic stone, shallow rectangular
      panel seams, darker base scuffs, slight water staining near lower edge,
      clean maintained public landscaping context.
    acceptance:
      - Reads as planter wall, not generic wall.
      - Lower edge has mild wear/staining.
      - Compatible with pale concrete family.

  - id: wall_planter_base_shadow_01
    filename: skymall_wall_planter_base_shadow_01.png
    size: 64x32
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: planter_architecture
    priority: medium
    intended_use: Lower planter base trim, contact-shadow band.
    prompt: >
      Create a seamless 64x32 pixel pale concrete planter base texture for a
      retro skymall. Warm stone face with darker bottom contact-shadow band,
      chunky chips near floor line, clean public-atrium material.
    acceptance:
      - Clear darker bottom edge.
      - Tiles horizontally.
      - Useful for planter bases and low walls.

  - id: ceiling_dark_soffit_concrete_plain_01
    filename: skymall_ceiling_dark_soffit_concrete_plain_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: none
      qa_preview: inspect as a 2x2 repeat; mottling has no obvious boundary
    category: ceiling
    priority: essential
    intended_use: Balcony undersides, overhangs, ceiling planes.
    prompt: >
      Create a seamless 64x64 pixel diffuse texture of dark shadowed concrete
      soffit for a skymall. Warm dark gray-brown, blocky mottling, broad painted
      shadow patches, subtle grime, clean public infrastructure, late-1990s
      hand-painted texture.
    acceptance:
      - Darker than wall concrete.
      - Not horror-grimy.
      - Works on large ceiling brush faces.

  - id: ceiling_dark_panel_grid_01
    filename: skymall_ceiling_dark_panel_grid_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel seams do not double at boundaries
    category: ceiling
    priority: high
    intended_use: Structured ceiling areas, underside of upper concourses.
    prompt: >
      Create a seamless 64x64 pixel dark ceiling panel texture for a retro
      futuristic skymall. Blue-gray dark concrete/composite panels, simple grid
      seams, subtle inset shading, chunky hand-painted pixels, public atrium
      underside material.
    acceptance:
      - Panel grid reads clearly.
      - Useful under walkways.
      - Compatible with recessed light textures later.

  - id: ceiling_recessed_shadow_strip_01
    filename: skymall_ceiling_recessed_shadow_strip_01.png
    size: 64x32
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: ceiling_trim
    priority: medium
    intended_use: Recessed ceiling bands, shadow strips under balconies.
    prompt: >
      Create a seamless 64x32 pixel dark recessed shadow strip for a skymall
      ceiling. Deep blue-gray center, warmer gray edges, hand-painted bevel
      shadow, chunky low-res pixels, designed for long underside trim brushes.
    acceptance:
      - Strong recessed-band illusion.
      - Tiles horizontally.
      - Useful for adding depth to flat ceilings.

  - id: ceiling_stair_underside_01
    filename: skymall_ceiling_stair_underside_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; diagonal panel marks continue across edges
    category: ceiling
    priority: medium
    intended_use: Underside of stair/escalator structures.
    prompt: >
      Create a seamless 64x64 pixel texture of the underside of a public
      escalator or stair structure in a futuristic skymall. Dark warm-gray
      concrete/composite panels, diagonal shadow impression, chunky seams,
      maintained but shadowed public infrastructure.
    acceptance:
      - Reads as underside material.
      - Dark but not black.
      - Compatible with escalator/stair geometry.

  - id: trim_pale_concrete_cap_01
    filename: skymall_trim_pale_concrete_cap_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim
    priority: essential
    intended_use: Wall caps, planter lips, balcony slab edges.
    prompt: >
      Create a seamless 64x16 pixel pale concrete cap trim for a retro
      futuristic skymall. Warm beige-gray stone, bright top edge, darker lower
      edge, chunky bevel illusion, small chips, clean public architecture.
    acceptance:
      - Top and bottom edge direction is clear.
      - Tiles horizontally.
      - Works as a brush edge finisher.

  - id: trim_pale_concrete_thick_lip_01
    filename: skymall_trim_pale_concrete_thick_lip_01.png
    size: 64x32
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim
    priority: high
    intended_use: Thick balcony lips, planter top edges, ledges.
    prompt: >
      Create a seamless 64x32 pixel thick pale concrete ledge trim for a
      skymall. Chunky hand-painted bevel, warm limestone-gray top face, darker
      underside shadow, small blocky chips, late-90s brush-based level texture.
    acceptance:
      - Reads as thick ledge/cap.
      - Strong bevel shading.
      - Useful on balcony and planter edges.

  - id: trim_dark_metal_rail_cap_01
    filename: skymall_trim_dark_metal_rail_cap_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_metal
    priority: essential
    intended_use: Glass railing caps, storefront edges, sign frame edges.
    prompt: >
      Create a seamless 64x16 pixel dark blue-gray metal rail cap texture for
      a retro skymall. Charcoal teal metal, bright upper pixel highlight, dark
      lower shadow, simple hand-painted edge wear, clean public infrastructure.
    acceptance:
      - Reads as narrow metal cap.
      - Tiles cleanly.
      - Works with glass railing textures.

  - id: trim_dark_metal_panel_seam_01
    filename: skymall_trim_dark_metal_panel_seam_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_metal
    priority: high
    intended_use: Storefront seams, metal panel borders, kiosk borders.
    prompt: >
      Create a seamless 64x16 pixel dark metal seam trim for a futuristic
      skymall. Blue-black metal strip, subtle center groove, chunky cyan-gray
      edge highlights, low-res hand-painted game texture.
    acceptance:
      - Thin and versatile.
      - Strong enough to divide panels.
      - Not rusty or militarized.

  - id: trim_warm_gold_accent_01
    filename: skymall_trim_warm_gold_accent_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_accent
    priority: medium
    intended_use: Storefront accent strips, premium mall details.
    prompt: >
      Create a seamless 64x16 pixel warm gold/ochre accent trim for a
      retro-futurist skymall. Muted yellow-gold painted metal or plastic, dark
      brown lower edge, simple chunky highlight, late-90s game texture style.
    acceptance:
      - Reads as muted gold accent, not shiny metal.
      - Useful for premium storefront or civic detail.
      - Not too saturated.

  - id: trim_cyan_inset_line_01
    filename: skymall_trim_cyan_inset_line_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded casing edges
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_light
    priority: high
    intended_use: Inset cyan architectural accent line.
    prompt: >
      Create a seamless 64x16 pixel cyan inset line trim for a futuristic
      skymall. Dark blue-gray casing, thin bright cyan center, teal edge pixels,
      simple low-res glow suggestion, subtle not cyberpunk-heavy.
    acceptance:
      - Reads as embedded sci-fi accent.
      - Tiles horizontally.
      - Usable sparingly on edges and signs.

  - id: trim_tile_grout_border_01
    filename: skymall_trim_tile_grout_border_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded floor-border edges
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_floor
    priority: medium
    intended_use: Floor borders, transitions between tile zones.
    prompt: >
      Create a seamless 64x16 pixel floor border trim for a skymall. Dark
      grout line with warm beige stone edges, chunky pixel wear, designed to
      separate floor tile zones in a late-1990s brush-based level.
    acceptance:
      - Useful as transition strip.
      - Matches warm floor tile palette.
      - Tiles cleanly.

  - id: trim_step_nosing_pale_01
    filename: skymall_trim_step_nosing_pale_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_stair
    priority: high
    intended_use: Stair nosing, step lips, platform edges.
    prompt: >
      Create a seamless 64x16 pixel pale stair nosing trim for a clean
      futuristic skymall. Beige-gray concrete edge, bright upper highlight,
      dark underside line, subtle wear, chunky hand-painted pixel style.
    acceptance:
      - Clearly marks a step edge.
      - Not industrial hazard tape.
      - Compatible with pale concrete and floor tile.

  - id: trim_safety_edge_muted_yellow_01
    filename: skymall_trim_safety_edge_muted_yellow_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded material bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: trim_stair
    priority: medium
    intended_use: Stair/escalator caution edges, transit-adjacent lips.
    prompt: >
      Create a seamless 64x16 pixel muted yellow public-space safety edge
      strip. Soft ochre-yellow line, dark blue-gray border, light scuffing,
      clean civic mall/transit feel, chunky late-90s pixel texture.
    acceptance:
      - Reads as caution/safety edge.
      - Does not look like industrial hazard stripes.
      - Works with escalator/stair materials.

  - id: glass_atrium_sky_reflective_01
    filename: skymall_glass_atrium_sky_reflective_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; cloud/reflection bands continue across edges
    category: glass
    priority: essential
    alpha_expected: true
    intended_use: Skylight panels, upper atrium windows.
    prompt: >
      Create a seamless 64x64 pixel stylized blue glass texture for a bright
      skymall atrium. Cyan-blue tint, chunky white cloud reflections, broad
      sky-blue bands, dark edge gradients, low-resolution hand-painted pixels,
      suitable for late-1990s low-poly game glass.
    acceptance:
      - Reads as bright sky-reflective glass.
      - Reflection shapes are chunky and abstract.
      - Tileable for repeated skylight panels.

  - id: glass_atrium_sky_reflective_02_lighter
    filename: skymall_glass_atrium_sky_reflective_02_lighter.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; cloud/reflection bands continue across edges
    category: glass
    priority: high
    alpha_expected: true
    intended_use: Brighter roof glass and sun-facing window panels.
    prompt: >
      Create a seamless 64x64 pixel lighter variant of stylized atrium glass
      for a futuristic skymall. Pale cyan-blue, large soft white cloud chunks,
      subtle sky gradient, bright daylight mood, chunky pixel-art texture for a
      1990s 3D game.
    acceptance:
      - Brighter than glass_atrium_sky_reflective_01.
      - Good for skylights and sunny windows.
      - Not photoreal.

  - id: glass_railing_cyan_smoke_01
    filename: skymall_glass_railing_cyan_smoke_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; reflection bands continue across edges
    category: glass
    priority: essential
    alpha_expected: true
    intended_use: Balcony railings, bridge railings.
    prompt: >
      Create a seamless 64x64 pixel cyan smoky glass railing texture for a
      retro-futurist skymall. Dark teal transparent glass, chunky horizontal
      highlights near top and bottom, blocky reflection bands, low-poly mall
      architecture style.
    acceptance:
      - Distinct from bright atrium glass.
      - Strong top/bottom highlight.
      - Works on narrow railing brush faces.

  - id: glass_shopfront_bluegray_01
    filename: skymall_glass_shopfront_bluegray_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; vertical reflections continue across edges
    category: glass
    priority: medium
    alpha_expected: true
    intended_use: Storefront windows and display glass.
    prompt: >
      Create a seamless 64x64 pixel blue-gray shopfront glass texture for a
      futuristic skymall. Darker than atrium glass, subtle cyan reflections,
      broad vertical reflection bands, low-res hand-painted display-window feel.
    acceptance:
      - Reads as storefront/display glass.
      - Dark enough to imply interior space behind it.
      - Pairs with dark metal trims.

  - id: glass_bridge_panel_edge_01
    filename: skymall_glass_bridge_panel_edge_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel edges do not double at boundaries
    category: glass
    priority: medium
    alpha_expected: true
    intended_use: Large glass panels on skybridges.
    prompt: >
      Create a seamless 64x64 pixel glass panel texture for elevated skymall
      skybridges. Cyan-blue transparent glass, thicker bright vertical and
      horizontal edge reflections, slight smoky tint, chunky late-90s pixel
      texture style.
    acceptance:
      - Feels like thick safety glass.
      - Works on repeated bridge panels.
      - Stronger edge lines than skylight glass.

  - id: metal_pale_aluminum_truss_01
    filename: skymall_metal_pale_aluminum_truss_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: none
      qa_preview: inspect as a 2x2 repeat; wear and bolt hints have no obvious boundary
    category: metal
    priority: essential
    intended_use: Skylight trusses, roof frames, pale structural beams.
    prompt: >
      Create a seamless 64x64 pixel pale painted aluminum texture for a
      futuristic skymall skylight truss. Warm off-white metal, simple blocky
      edge wear, subtle bolt hints, clean civic atrium structure, late-1990s
      hand-painted texture.
    acceptance:
      - Feels bright, civic, and architectural.
      - Not spaceship armor.
      - Usable on beams and trusses.

  - id: metal_pale_aluminum_joint_plate_01
    filename: skymall_metal_pale_aluminum_joint_plate_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; seams and bolt clusters do not double
    category: metal
    priority: medium
    intended_use: Beam joints, truss plates, roof connection points.
    prompt: >
      Create a seamless 64x64 pixel pale aluminum joint plate texture for a
      skymall roof structure. Off-white painted metal panels, chunky bolt
      clusters, simple seam lines, hand-painted low-res public architecture.
    acceptance:
      - Reads as structural joint/plate material.
      - Bolt hints are chunky, not photoreal.
      - Compatible with pale truss texture.

  - id: metal_dark_bluegray_panel_01
    filename: skymall_metal_dark_bluegray_panel_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel seams do not double at boundaries
    category: metal
    priority: essential
    intended_use: Storefront frames, rail supports, underside technical panels.
    prompt: >
      Create a seamless 64x64 pixel dark blue-gray metal panel texture for a
      retro-futurist skymall. Charcoal teal base, broad painted highlights,
      simple rectangular panel seams, clean public-infrastructure material,
      chunky pixel style.
    acceptance:
      - Reads as dark architectural metal.
      - Not rusty or military.
      - Good for storefront and railing supports.

  - id: metal_dark_ribbed_panel_01
    filename: skymall_metal_dark_ribbed_panel_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; ribs continue across edges
    category: metal
    priority: high
    intended_use: Storefront side panels, escalator sides, service panels.
    prompt: >
      Create a seamless 64x64 pixel dark ribbed metal/composite panel texture
      for a futuristic mall. Horizontal or vertical blue-black ribs, muted teal
      highlights, subtle scuffs, chunky hand-painted late-90s game texture.
    acceptance:
      - Ribbing is clear.
      - Useful for commercial/technical panels.
      - Does not dominate the bright civic palette.

  - id: metal_blackblue_frame_01
    filename: skymall_metal_blackblue_frame_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: none
      qa_preview: inspect as a 2x2 repeat; edge highlights have no obvious boundary
    category: metal
    priority: medium
    intended_use: Window frames, sign frames, shopfront frame pieces.
    prompt: >
      Create a seamless 64x64 pixel black-blue frame metal texture for a
      late-1990s skymall. Very dark navy/charcoal metal, chunky edge highlights,
      subtle bevel suggestions, clean commercial architecture, no rust.
    acceptance:
      - Dark enough for frame contrast.
      - Clean and architectural.
      - Works with glass textures.

  - id: balcony_face_pale_panel_01
    filename: skymall_balcony_face_pale_panel_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; horizontal panel rows keep normal weight
    category: balcony
    priority: essential
    intended_use: Upper-level balcony fascia and visible slab faces.
    prompt: >
      Create a seamless 64x64 pixel pale concrete balcony face texture for a
      futuristic skymall. Large horizontal public-architecture panels, warm
      limestone-gray surface, darker lower edge shadows, subtle hand-painted
      wear, chunky pixel texture.
    acceptance:
      - Reads as balcony/slab side.
      - Strong horizontal orientation.
      - Matches pale concrete family.

  - id: balcony_under_shadow_edge_01
    filename: skymall_balcony_under_shadow_edge_01.png
    size: 64x32
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded balcony-edge bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: balcony
    priority: high
    intended_use: Lower edge under balcony lips.
    prompt: >
      Create a seamless 64x32 pixel balcony underside edge texture for a retro
      skymall. Pale concrete upper lip with dark gray-brown shadow underneath,
      chunky bevel illusion, clean but shadowed public atrium construction.
    acceptance:
      - Useful as transition between balcony face and soffit.
      - Strong shadow band.
      - Tiles horizontally.

  - id: bridge_side_pale_concrete_01
    filename: skymall_bridge_side_pale_concrete_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; horizontal seams keep normal weight
    category: bridge
    priority: high
    intended_use: Side faces of elevated walkways and skybridges.
    prompt: >
      Create a seamless 64x64 pixel pale concrete side texture for skymall
      elevated walkways. Warm gray-beige panel faces, subtle horizontal seams,
      clean civic transit-bridge feeling, low-res hand-painted game texture.
    acceptance:
      - Reads as elevated walkway side.
      - Compatible with bridge deck flooring.
      - Not too decorative.

  - id: bridge_support_dark_joint_01
    filename: skymall_bridge_support_dark_joint_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; bracket seams continue across edges
    category: bridge
    priority: medium
    intended_use: Bridge support brackets and structural joints.
    prompt: >
      Create a seamless 64x64 pixel dark blue-gray bridge support joint texture
      for a futuristic skymall. Clean metal/composite brackets, simple chunky
      seam lines, muted teal highlights, public infrastructure, late-90s low-poly
      texture style.
    acceptance:
      - Reads as support/bracket material.
      - Good for small structural brush pieces.
      - Not military or industrial.

  - id: escalator_tread_dark_ribbed_01
    filename: skymall_escalator_tread_dark_ribbed_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; tread ribs continue across edges
    category: circulation
    priority: essential
    intended_use: Escalator treads, stair treads, sloped moving-walkway surfaces.
    prompt: >
      Create a seamless 64x64 pixel dark ribbed escalator tread texture for a
      late-1990s skymall. Charcoal and blue-gray horizontal ribs, chunky pixel
      bands, durable public-transit material, readable on sloped brush faces.
    acceptance:
      - Strong directional ribbing.
      - Reads as tread material from distance.
      - Loops cleanly.

  - id: stair_tread_stone_01
    filename: skymall_stair_tread_stone_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: continuation
      qa_preview: inspect as a 2x2 repeat; wear bands continue across edges
    category: circulation
    priority: high
    intended_use: Non-escalator stairs, broad public steps.
    prompt: >
      Create a seamless 64x64 pixel warm stone stair tread texture for a bright
      skymall. Beige-gray public stone, subtle horizontal wear bands, chunky
      scuffing near front edge, clean civic mall style, late-90s pixel art.
    acceptance:
      - Reads as stair surface.
      - Compatible with main warm floor tile.
      - Slight directional wear.

  - id: escalator_side_pale_composite_01
    filename: skymall_escalator_side_pale_composite_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; panel seams do not double at boundaries
    category: circulation
    priority: essential
    intended_use: Escalator cheek walls and side panels.
    prompt: >
      Create a seamless 64x64 pixel pale escalator side panel texture for a
      clean futuristic skymall. Off-white composite, subtle gray scuffs, simple
      panel seams, broad bevel shading, chunky late-90s game texture.
    acceptance:
      - Contrasts with dark tread texture.
      - Works on long sloped side faces.
      - Clean public-space material.

  - id: escalator_handrail_blackblue_01
    filename: skymall_escalator_handrail_blackblue_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded handrail edges
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: circulation_trim
    priority: high
    intended_use: Escalator handrails and dark rounded rail strips.
    prompt: >
      Create a seamless 64x16 pixel dark black-blue escalator handrail texture
      for a futuristic skymall. Soft rubber/plastic impression, chunky highlight
      along top edge, dark teal shadow, low-res hand-painted style.
    acceptance:
      - Reads as dark handrail.
      - Tiles horizontally.
      - Works with pale escalator side panel.

  - id: escalator_base_dark_panel_01
    filename: skymall_escalator_base_dark_panel_01.png
    size: 64x32
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded panel bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: circulation
    priority: medium
    intended_use: Escalator base panels and machinery-adjacent side bands.
    prompt: >
      Create a seamless 64x32 pixel dark escalator base panel texture for a
      retro skymall. Blue-gray metal/composite, simple service panel seams,
      subtle scuffs, public transit infrastructure feel, chunky pixels.
    acceptance:
      - Works at bottom/top of escalator assemblies.
      - Dark but clean.
      - Not industrial machinery-heavy.

  - id: landing_edge_transition_01
    filename: skymall_landing_edge_transition_01.png
    size: 64x32
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded transition bands
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: circulation_trim
    priority: medium
    intended_use: Landing transitions between escalator/stair and floor.
    prompt: >
      Create a seamless 64x32 pixel landing transition texture for a skymall.
      Warm stone floor meeting dark metal tread edge, clean public threshold,
      chunky grout and trim line, late-90s brush texture style.
    acceptance:
      - Clearly bridges floor and escalator materials.
      - Useful on landing brush faces.
      - Matches warm stone and dark tread palettes.

  - id: utility_smooth_offwhite_panel_01
    filename: skymall_utility_smooth_offwhite_panel_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: none
      qa_preview: inspect as a 2x2 repeat; scuffs have no obvious boundary
    category: utility
    priority: medium
    intended_use: Blank filler panels, service walls, simple interior surfaces.
    prompt: >
      Create a seamless 64x64 pixel smooth off-white composite wall panel
      texture for a clean futuristic skymall. Very subtle warm-gray variation,
      a few broad scuffs, minimal seams, chunky hand-painted low-res pixels.
    acceptance:
      - Quiet filler material.
      - Not pure flat color.
      - Does not compete with hero surfaces.

  - id: utility_gray_service_panel_01
    filename: skymall_utility_gray_service_panel_01.png
    size: 64x64
    tileable: true
    tiling:
      mode: repeat_2d
      edge_contract: left/right and top/bottom edges must match; corners wrap cleanly
      edge_phase: shared_seam
      qa_preview: inspect as a 2x2 repeat; access-panel seams do not double at boundaries
    category: utility
    priority: medium
    intended_use: Service doors, maintenance panels, back-of-house walls.
    prompt: >
      Create a seamless 64x64 pixel gray-blue service panel texture for a
      futuristic public mall. Simple rectangular access panel seams, muted
      gray-blue composite, subtle scuffs, clean maintained infrastructure,
      chunky retro game texture.
    acceptance:
      - Reads as service/utility panel.
      - Not gritty industrial.
      - Useful in secondary areas.

  - id: utility_floor_wall_baseboard_01
    filename: skymall_utility_floor_wall_baseboard_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded baseboard edges
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: utility_trim
    priority: high
    intended_use: Baseboards where walls meet floors.
    prompt: >
      Create a seamless 64x16 pixel baseboard trim for a skymall wall/floor
      junction. Pale beige-gray upper edge, darker contact shadow, simple clean
      public-building finish, chunky pixel-art texture.
    acceptance:
      - Useful at wall-floor intersections.
      - Strong contact-shadow line.
      - Tiles horizontally.

  - id: utility_expansion_joint_01
    filename: skymall_utility_expansion_joint_01.png
    size: 64x16
    tileable: true
    tiling:
      mode: repeat_u
      edge_contract: left/right edges must match; top/bottom are bounded joint edges
      edge_phase: continuation
      qa_preview: inspect as a 3x1 horizontal repeat
    category: utility_trim
    priority: low
    intended_use: Expansion joints, floor seams, architectural breaks.
    prompt: >
      Create a seamless 64x16 pixel expansion joint texture for a futuristic
      skymall floor or wall. Thin dark rubber-like seam between pale concrete
      edges, subtle scuffing, clean public architecture, late-90s low-res style.
    acceptance:
      - Reads as architectural seam.
      - Simple and reusable.
      - Does not look like a hazard marking.
