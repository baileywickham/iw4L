# Custom content: research notes (2026-10-02)

Nothing here needs to run in the retail game; IW4L can read loose formats of its own.

## What exists

- **No official MW2 mod tools** ([GameSpot](https://www.gamespot.com/articles/infinity-ward-teases-pc-modern-warfare-2-mod-tools/1100-6243532/)). Every community pipeline starts from the CoD4 mod tools.
- **CoD4 mod tools** (Windows): Radiant → `cod4map` → `cod4rad` → path connection done *by the game* (`sp_tool.exe +set g_connectpaths 2 +devmap <map>`, [compile script](https://github.com/promod/CoD4-Mod-Tools/blob/master/bin/CoD4CompileTools/cod4compiletools_compilebsp.bat)). Path nodes are hand-placed `node_pathnode` / `node_cover_*` entities; rules of thumb ≤128u links, nodes ~24u off the floor ([zeroy wiki](https://wiki.zeroy.com/index.php?title=Call_of_Duty_4%3A_SP_-_Basic_AI_Paths)).
- **IW4x port pipeline** (GPL-3.0, Windows, in-game): [iw3x-port](https://github.com/SnowyWhite/iw3x-port) exports from CoD4, the [iw4x-client](https://github.com/iw4x/iw4x-client) ZoneBuilder links a `.ff`; [map porting utility](https://github.com/iw4x/iw4x-map-porting-utility). MP maps only ([wiki](https://github.com/Emosewaj/IW4x/wiki/Create-a-map)); ZoneBuilder can't load GameWorldSp from disk. [iw4-open-formats](https://github.com/iw4x/iw4-open-formats) is the best reference for on-disk map asset content (no GameWorldSp/Mp).
- **OpenAssetTools** ([support table](https://github.com/Laupetin/OpenAssetTools/blob/main/docs/SupportedAssetTypes.md)): IW4 map assets (clipMap, ComWorld, GameWorld, GfxWorld, FxWorld) neither dump nor load; XModel/XAnim/Material/Techset/Weapon/RawFile do.
- **Precedent:** [LJW-Dev OAT fork](https://github.com/LJW-Dev/OpenAssetTools) compiles glTF → T6 GfxWorld/ClipMap/ComWorld/GameWorldMp/MapEnts with in-code BSP (MP, no path nodes).
- **ZoneTool** ([repo](https://github.com/ZoneTool/zonetool)): injected DLL, Windows.
- **SP clients:** iw4x-sp archived 2026-09. No custom SP/Spec Ops maps found anywhere.
- **Model/anim export** (macOS-capable Blender add-ons): [BlenderCODTool](https://github.com/Valerie-Bosco/BlenderCODTool), [pv_blender_cod](https://github.com/prov3ntus/pv_blender_cod), [Cast](https://github.com/dtzxporter/cast).
- **Procedural level precedent** (not LLM, not IW): OBLIGE, [aa2map](https://aa2map.sourceforge.net/), [DeepMind Lab text levels](https://github.com/google-deepmind/lab/blob/master/docs/developers/creating_levels/level_generation.md).

## Plan for IW4L-only content (all native Rust, macOS)

1. **Missions on retail maps:** mission manifest (base map + GSC + entity file) and a loose addon-entity file merged like a mission zone's AddonMapEnts.
2. **Auto path nodes:** sample walkable floor from the ClipMap (or Recast navmesh), nodes every 64–128u, links by trace (≤128u, step/height checks), cover nodes on wall edges, then node tree + vis bytes. Also gives AI on MP maps.
3. **Minimal world compiler:** text brushes / glTF → ClipMap + BSP, GfxWorld surfaces with flat/vertex lighting (lightmaps later), lights, MapEnts, auto nodes; loose files, no fastfile writing. Generator scripts (room/corridor grammar, heightfields) produce the input.
4. **Assets:** glTF → XModel loader in IW4L; weapons as text tweaks of retail defs; retail materials/techsets only.

## Legal

Activision has issued C&Ds (IW4x/X Labs 2023, H2M 2024). Never distribute `.ff` files or extracted assets; share only scripts, entity patches and original content that require the user's own game files. GPL tools are format references only.
