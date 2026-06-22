## Skymall Texture Bible for TrenchBroom / Retro 3D Level Textures

The target style is a **late-1990s low-poly 3D game interpretation of a futuristic “skymall”**: an elevated public mall, transit hub, and civic atrium designed to feel pleasant, restorative, and optimistic, but built with the material economy of old PC/console 3D games. The textures should feel like they belong in a Quake-era or late-90s level editor pipeline, but the subject matter should be brighter, more humane, and more civic than gothic, industrial, or military spaces.

This is not photorealism. It is not modern PBR realism. It is **chunky hand-painted pixel texture work applied to simple brush geometry**.

The environment should read as:
**post-pandemic public-space optimism rendered through a late-90s game engine.**

Generous daylight, greenery, clean air, water features, open circulation, comfortable lounge areas, wellness signage, bright atrium glass, and futuristic wayfinding should coexist with blocky stone, obvious tiling, hard-edged geometry, and low-resolution pixel-painted surfaces.

## Core visual identity

The texture language should combine four ideas:

First, **civic sky-architecture**: pale structural concrete, warm stone floors, glass railings, skylight frames, elevated walkways, atrium bridges, modular planters, and wayfinding signs.

Second, **retro-futurist commercial space**: mall storefronts, kiosks, directory screens, glowing cyan signage, chunky corporate branding, fake shop interiors, product displays, posters, and suspended banners.

Third, **biophilic public-space design**: vines, shrubs, planter beds, water walls, fountain jets, soft seating, coffee-shop signage, wellness posters, and a feeling of intentional comfort.

Fourth, **late-1990s 3D game texture constraints**: low resolution, aggressive readability, flat albedo color, simple material ramps, repeated tiles, exaggerated seams, and textures that still work when mapped to blocky brushwork.

The result should feel like a lost optimistic sci-fi expansion pack for a Quake-era engine: cleaner and sunnier than Quake, more architectural than a cyberpunk alley, more public and social than a space station, but still unmistakably retro-game texture work.

## Resolution and format assumptions

Most textures should be designed at **128×128 pixels**.

Use **64×64** for small repeating trims, simple utility panels, compact floor or wall variants, and textures that need a deliberately chunkier old-game read.

Use **32×32** only for tiny trims, lights, bolts, icon panels, flower clusters, and simple decals.

Use **64×128** or **128×64** for vertical signs, hanging banners, posters, storefront headers, waterfall strips, vines, and directional signage.

Use **128×128** for special hero textures such as the atrium floor mosaic, a large sign, a shop display panel, or a skyline/window backdrop.

Use **256×256** only for rare visual centerpieces that need to hold attention up close: a large decorative tile field, a major floor medallion, a mural-like shop feature, or another one-off focal surface. These should be non-tiling unless the prompt explicitly says otherwise.

Avoid textures larger than 128×128 except for named hero exceptions. A larger texture must justify its memory cost by serving a specific focal surface, not a general wall or floor tile.

Do not create texture sheets or atlases. Author each TrenchBroom material as an individual texture file. A sign, store panel, poster, trim, tile, glass panel, or facade should be its own PNG rather than a region inside a larger bitmap.

Optional normal and specular maps should use same-stem sibling files for the individual diffuse texture: `[name].png`, `[name]_n.png`, and `[name]_s.png`.

All textures should be usable as **diffuse/albedo-first game textures**. They should read as finished material color, not as pre-lit surfaces. If specular or normal maps are generated, they should support the pixel-art look rather than override it.

Use `skymall-reference-image.png` for palette, subject matter, material vocabulary, and scene mood only. Do not copy its lighting, shadows, camera perspective, or composed scene into texture bitmaps.

## Texture tool pass

Use `tools/texture-tool` after image generation produces source art. Treat it as a finishing and normalization step for PostRetro / TrenchBroom texture bundles, not as a replacement for art-direction prompts or acceptance review.

The tool defaults to **128×128** output. It enforces exact dimensions, individual files instead of sheets or atlases, flat diffuse cleanup, restrained specular maps, and normal maps. It outputs same-stem siblings for each material: `stem.png`, `stem_s.png`, and `stem_n.png`.

Use `process` for one source texture. Use `batch` with a manifest for many textures. Use the `contact` binary for quick contact-sheet QA before accepting a set.

## Lighting and shading separation

Bitmap textures should be flat from a lighting and shading perspective.

Diffuse textures carry albedo: material color, pixel clusters, seams, stains, wear, labels, grout, signage, foliage shapes, and decorative motifs. They should not carry directional lighting, baked shadows, ambient-occlusion corners, contact shadows, rim highlights, or painted light falloff.

Lighting depth should come from the environment: lightmaps, dynamic lights, SH irradiance, normal maps, and specular maps. The texture set should leave room for those systems to supply shape, shadow, gloss, and surface response.

Use value changes to describe material variation, not illumination. A concrete tile can have mottling and chips; it should not have a bright top edge and dark bottom edge unless those pixels represent a physical trim color or separate material band.

For TrenchBroom usage, assume the textures will be applied to **brush-based architecture**: square floors, thick walls, stairs, escalator forms, columns, planters, low retaining walls, shop facades, and simple modular props. The textures should tile cleanly where needed and should not rely on dense unique UV layouts.

## Pixel-art rendering rules

The textures should look hand-painted at low resolution.

Use **chunky pixel clusters**, not fine noise. Avoid photographic source texture artifacts. Avoid AI-smoothed detail. Avoid painterly blur. Avoid high-frequency speckles that collapse into mush.

Edges should often be slightly exaggerated: grout lines, panel seams, trim strips, tile borders, concrete chips, and bevel cues should be visible at a distance.

Use broad value shapes. A 64×64 texture should have only a few clear material zones, not dozens of micro-details.

Keep diffuse lighting flat. Do not paint shadows, highlights, darkened corners, contact occlusion, or light direction into the bitmap. Use clear albedo zones and crisp material marks so engine lighting can add depth without fighting baked-in shade.

Normals, if generated, should be restrained. They should reinforce tile seams, panel grooves, stone chips, ribbed escalator treads, and subtle bevels. Specular maps should identify glossy or dull regions without implying light direction. Neither map should create modern procedural material complexity.

## Palette

The general palette should be brighter and cleaner than most 90s shooters.

Use these color families:

Warm civic stone: cream, sand, beige, tan, warm gray, pale ochre.

Cool infrastructure: blue-gray, slate, charcoal, muted teal, dark cyan.

Glass and sky: saturated sky blue, pale cyan, deep blue reflections, white cloud chunks.

Signage and screens: cyan, aqua, teal, green-blue, occasional lime green.

Greenery: deep green, moss green, yellow-green, muted olive, small red/yellow flower pixels.

Accent materials: muted gold, amber signage, dark blue-black storefront panels, soft teal upholstery.

Avoid excessive black except in signage, screens, store interiors, and tech trim. Avoid red as a dominant color. Avoid gritty brown industrial palettes unless used for soil, planter mulch, or subtle weathering.

The feeling should be: **sunlit, breathable, clean, retro-futurist, civic, and slightly corporate.**

## Architectural material language

### Pale structural concrete / civic stone

This is the dominant mall material. It should feel like a hybrid of poured concrete, limestone, and polished public-infrastructure stone. It should be clean, but not sterile.

Use mottled beige-gray blocks, subtle stains, chipped corners, darker seam pixels, and broad flat material patches. It should work on columns, planter bases, balcony faces, stair sidewalls, wall panels, and overhead structures.

It should not look like medieval stone, dungeon rock, bunker concrete, or photoreal scanned concrete. It should feel like a premium mall/transit atrium material rendered with 90s game limitations.

Good descriptors:
**warm pale concrete, civic limestone, chunky pixel mottling, clean public architecture, low-poly mall structure, flat pixel edge wear.**

### Warm tiled floors

The main floor texture should be a readable grid of warm stone tiles. It should have strong grout lines and slightly varied tile colors. Tiles can be square or rectangular, but should feel organized and maintained.

The floor must support large open plazas without becoming visually dead. Use subtle diagonal or broken color variation inside each tile. Keep the contrast between tile and grout high enough to read in perspective.

Good descriptors:
**late-90s plaza tile, beige stone mall floor, chunky grout, pixel-painted tile variation, clean but worn public walkway.**

### Decorative mosaic floor

The skymall needs a focal “atrium medallion” texture: an inlaid mosaic suggesting sky, water, clouds, or civic optimism. It should use teal, blue, cream, tan, and dark grout.

The shapes should be chunky and radial. Do not make it delicate or high-resolution. It should look like a 1998 game texture trying to represent a fancy floor in only a few pixels.

Good descriptors:
**chunky civic mosaic, blue-green atrium medallion, low-res inlaid tile, pixel-art radial floor design.**

### Dark underside concrete

Balcony undersides, overhangs, ceiling planes, and recessed shop soffits should use a darker gray-brown concrete material. This creates material contrast against the bright atrium palette.

It should be blocky and mottled, with occasional ceiling lights embedded into it. Do not bake underside darkness as a lighting gradient; keep the bitmap as a flat darker material.

Good descriptors:
**dark concrete underside, mall overhang material, dark warm-gray public infrastructure, chunky low-res grime.**

### Skylight frame metal

The roof lattice and atrium trusses should use pale metal, not black steel. Think powder-coated aluminum or painted structural beams. The texture should include blocky edge color bands and slightly darker bolt/seam suggestions.

Good descriptors:
**painted aluminum skylight truss, pale sci-fi structural metal, chunky low-poly beam texture, optimistic atrium roof frame.**

### Glass

Glass should be stylized, not realistic. It should include cyan-blue tinting, broad reflection bands, cloud-like shapes, and dark frame-adjacent color bands.

Two glass families are needed:

Atrium glass: bright, sky-reflective, airy.

Railing glass: darker, more cyan, smoky, with thick edge-color bands.

Good descriptors:
**pixelated blue glass, stylized sky reflections, cyan transparent railing, low-res reflective atrium window.**

## Signage and interface language

The signage style should feel like **late-90s corporate futurism**: blocky digital type, cyan LED screens, black panels, simplified icons, and optimistic lifestyle slogans.

Text does not need to be fully legible, but major words can be implied or readable when useful: “SKYMALL,” “DIRECTORY,” “LIVE WORK SHOP PLAY,” “BREATHE,” “LEVEL 2,” “TECH DEPOT,” “CLOUD NINE CAFE.”

Use black or very dark blue backgrounds with cyan, teal, lime, and white pixel text. Add simple screen borders, scanlines, and chunky UI blocks.

Signage should feel functional and public: wayfinding, directory panels, transit signs, storefront signs, wellness posters, and civic banners.

Good descriptors:
**cyan pixel wayfinding, retro LED directory screen, 1990s mall UI panel, black glass signboard, chunky corporate future typography.**

## Storefront language

Storefronts should be modular and easy to apply to brush geometry. Think “one texture can fake a whole shop entrance.”

### Tech storefronts

Dark ribbed panels, blue-black frames, cyan display screens, gold/yellow block letters, and simplified product shelves. The tech shop should feel like a mall electronics store filtered through old sci-fi UI design.

Good descriptors:
**dark ribbed tech storefront, pixel electronics display wall, cyan product screens, chunky gold shop lettering, late-90s sci-fi mall shop.**

### Cafe and lifestyle shops

Use softer blues, cream panels, coffee icons, sky/cloud motifs, plant accents, and warm trim. These spaces should communicate comfort, not aggression.

Good descriptors:
**sky-blue cafe poster, cloud coffee icon, soft public-space branding, low-res lifestyle mall signage.**

### Garden / wellness shops

Use green, amber, dark teal, wood-like trim, and plant silhouettes. Keep it stylized and symbolic.

Good descriptors:
**pixel zen garden sign, indoor plant shop facade, green amber wellness retail panel.**

## Biophilic texture language

Greenery is critical. It is what separates the scene from a sterile sci-fi mall.

Use alpha-card textures for shrubs, vines, broadleaf plants, and hanging foliage. Leaves should be chunky clusters, not detailed botanical illustrations.

Planters should include dark soil or mulch textures, plus dense shrub cards with occasional flower pixels. Vines should hang over balcony ledges and planter walls.

The greenery should look intentionally designed into the architecture, not abandoned overgrowth. This is not post-apocalyptic vegetation. It is maintained, curated, and welcoming.

Good descriptors:
**curated indoor greenery, chunky pixel shrub clusters, hanging balcony vines, biophilic public atrium foliage, maintained planter beds.**

## Water and wellness textures

Water features should be stylized and readable: waterfall walls, fountain jets, shallow pools, and blue reflective surfaces.

The water should feel like a civic/public wellness feature. It should be clean and calming, with cyan, teal, white, and dark blue pixel streaks.

Avoid realistic water simulation. Use old-game animated-water logic: tiling streaks, scrolling bands, bright white foam/detail pixels, and simple transparency.

Good descriptors:
**retro pixel waterfall, cyan wellness water wall, low-res fountain spray, chunky blue reflective pool, public atrium water feature.**

## Furniture and comfort language

Furniture should look modular, durable, and comfortable. The scene needs teal lounge seating, benches, kiosk bases, low tables, and public rest zones.

Upholstery should use broad teal and blue-green color blocks, simple seam lines, and darker edge wear. It should not look like plush photoreal fabric. It should look like a 90s game texture representing premium public seating.

Good descriptors:
**teal modular lounge upholstery, chunky public seating texture, pixel-painted vinyl fabric, clean mall bench material.**

## Kiosk and prop material language

Kiosks, directory stands, terminals, and public interface props should use muted gray-green casing materials with cyan screen accents. They should feel durable and civic, more like transit infrastructure than consumer electronics.

Use simple panel lines, bolts, beveled corners, and glowing screen inserts.

Good descriptors:
**public information kiosk casing, muted sci-fi transit terminal, cyan glowing interface, blocky gray-green console panel.**

## TrenchBroom-specific texture guidance

Textures should be brush-friendly.

Each texture should stand alone as a file that TrenchBroom can browse and assign directly. Do not depend on sheet coordinates, atlas packing, or sub-rect selection.

They should tile cleanly on square surfaces and tolerate being stretched or rotated. Important seams should align to the pixel grid. Avoid perspective baked into tiling textures unless the texture is intended as a poster, sign, or fake interior panel.

For floors and walls, strong edge seams and grout lines are useful because they give simple brush geometry scale.

For trims, use narrow textures such as **64×16**, **64×32**, or **128×16**. These can define edges, caps, borders, ledges, stair lips, sign frames, and light strips.

For signage and storefronts, use non-tiling textures with clear borders so they can be placed on flat brush faces like decals or panels.

Use texture categories like these:

`skymall_floor_*`
`skymall_wall_*`
`skymall_concrete_*`
`skymall_glass_*`
`skymall_trim_*`
`skymall_sign_*`
`skymall_store_*`
`skymall_foliage_*`
`skymall_water_*`
`skymall_light_*`
`skymall_kiosk_*`

## What makes the style distinct

The style is not simply “pixel art textures.” It is specifically:

A bright, optimistic public-space environment built with old 3D game constraints.

It uses **mall and transit architecture**, not castles, factories, hangars, or corridors.

It uses **clean civic materials**: limestone-like concrete, warm tile, glass, metal railings, planter stone, and modular public furniture.

It uses **post-pandemic comfort cues**: daylight, plants, water, lounges, open circulation, wellness posters, clean-air branding, and “come back to public life” messaging.

It uses **retro-futurist wayfinding**: cyan screens, black signboards, blocky LED text, mall directories, hanging banners, shop signs, and optimistic lifestyle slogans.

It uses **low-resolution hand-painted material economy**: every texture must be readable, chunky, tileable, and useful on simple brush geometry.

The core phrase is:

**A humane, sunlit, biophilic sky-mall atrium as imagined by a late-1990s 3D game texture artist.**

## Global negative guidance

Avoid photorealism.
Avoid modern Unreal Engine material complexity.
Avoid AI-smoothed painterly textures.
Avoid tiny procedural noise.
Avoid grunge-heavy dystopian cyberpunk.
Avoid medieval stone dungeon language.
Avoid military sci-fi corridors.
Avoid pristine Apple Store minimalism.
Avoid high-resolution PBR realism.
Avoid horror atmosphere.
Avoid overgrown abandoned mall decay.
Avoid hard-edged brutalism as the dominant mood.

The environment should be clean, public, bright, and optimistic, but still visibly constructed from low-res retro game textures.

## Reusable master prompt fragment

Use this at the top of texture-generation prompts:

“Create a low-resolution hand-painted pixel-art texture for a late-1990s low-poly 3D game environment, intended for brush-based level design in TrenchBroom. The setting is a futuristic ‘skymall’: a bright elevated public mall and transit atrium with post-pandemic civic design cues, biophilic architecture, clean-air wellness branding, modular public seating, glass railings, warm stone floors, pale concrete, cyan wayfinding screens, and optimistic retro-futurist signage. The texture should be chunky, readable, tile-friendly, and diffuse/albedo-first, with broad pixel clusters, flat lighting, simple material color ramps, clear seams, and no photorealistic detail. Do not paint directional lighting, shadows, ambient-occlusion corners, contact shadows, or highlights into the bitmap; leave lighting depth to lightmaps, normal maps, specular maps, and environment lighting. It should look like a texture made for a 1998 PC game, but depicting a clean, humane, sunlit futuristic public space.”

## Example material prompt pattern

For any specific texture, use this structure:

“Create a [SIZE] pixel texture of [MATERIAL], for a late-1990s low-poly 3D game skymall environment. It should show [KEY FEATURES]. Style: chunky hand-painted pixel art, diffuse/albedo-first, flat lighting, tile-friendly, readable at low resolution, broad color blocks, simple material color ramps, no painted highlights or shadows, no photorealism, no modern PBR material complexity. Palette: [PALETTE]. Intended use in TrenchBroom: [SURFACE / BRUSH USE].”

Example:

“Create a 128×128 pixel seamless texture of warm beige stone mall flooring for a late-1990s low-poly 3D game skymall environment. It should show square public-atrium floor tiles with chunky dark grout, subtle tan and cream variation, small hand-painted chips, and clean civic polish. Style: chunky hand-painted pixel art, diffuse/albedo-first, flat lighting, tile-friendly, readable at low resolution, broad color blocks, simple material color ramps, no painted highlights or shadows, no photorealism, no modern PBR material complexity. Palette: cream, beige, tan, warm gray, muted ochre. Intended use in TrenchBroom: large atrium floors, upper walkways, shop thresholds.”
