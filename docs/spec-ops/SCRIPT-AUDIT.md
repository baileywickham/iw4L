# GSC script audit (Spec Ops)

Which scripts `iw4sp.exe` runs by name that the zones carry but our SP program
did not compile. Rerun with `iw4l gsc-audit all` (or `gsc-audit <so_mission>...`):
headless, ~2 s per mission; it walks `common_mp`, SP `common`, the base map and
the mission zone, compiles the program the way a match install does
(`Iw4SpStartup`), prints the gaps and writes `<artifacts>/gsc-audit/<mission>.tsv`
(module, zone, compiled, engine-named). In a live match, `IW4L_GSC_MODULES_LOG=1`
logs every zone module as `compiled`/`not compiled` at install, and the engine
warns once per name when it asks for a function the program lacks
(`gsc: engine called … which is not in the program`).

## What the engine names (`iw4sp.exe` strings)

- `maps/%s` (level `main`), `codescripts/delete::main`, `codescripts/struct::initstructs`/`createstruct`.
- `aitype/%s` → `main`, `spawner`, `precache` for every `actor_*` classname.
- `animscripts/%s` → `main`, `end_script` per AI state: `init combat cover_arrival cover_crouch
  cover_left cover_prone cover_right cover_stand death flashed grenade_cower grenade_return_throw
  move pain reactions scripted stop`; `animscripts/scripted`.
- `animscripts/%s/%s` per species (`aitype` sets `self.type`): `dog/dog_{init combat death flashed
  move pain reactions scripted stop}`, `civilian/civilian_{the human states, grenade_response}`.
- `animscripts/traverse/%s` from a negotiation node's `animscript`.
- A turret weapon's `script` (`saw/stand|crouch|prone` in SP common, `hummer_turret/minigun_stand`
  in boneyard) — run when an AI mans a turret (not implemented yet).
- No `CodeCallback_*` in SP; `character/*` and `xmodelalias/*` are reached from aitype scripts at compile time.

## Gaps before the fix (c31e3b3)

| module | missions |
|---|---|
| `animscripts/civilian/civilian_*` (16; `init`/`move` already reachable on airport and oilrig) | killspree_favela, juggernauts_favela, defuse_favela_escape, download_arcadia, intel_boneyard (16 each); escape_airport, assault_oilrig, takeover_oilrig (14) |
| `animscripts/saw/{common,stand,crouch,prone}` (turret weapon scripts) | all 23 |
| `animscripts/hummer_turret/minigun_{stand,code}` | intel_boneyard, chopper_invasion, defense_invasion, killspree_invasion |
| `animscripts/technical/{stand,common}` | defuse_favela_escape |
| `animscripts/traverse/stairs_{up,down}` | all 23 — the zone sources are broken (missing `;`), dropped at compile, as the game never uses them |
| `animscripts/combat_say`, `animscripts/scripted/truckride_backoftruck`, `animscripts/civilian` | all 23 (dead or reached through `civilian/*`) |

Civilians also ran the human animscripts: `animscript_module` mapped only dogs.
Not missing: `reactions`, every other human/dog state, every aitype present. Uncompiled
`character/character_sp_opforce_*` (3 missions) and `character_sp_juggernaut*` (2) are not used
by any aitype in those zones. `animscripts/animmode` (patrol, stealth, `_idle`) is reached through
`maps/_anim` (`animcustom`), so the forest patrol bind pose is not a missing script.

## Fix

`Iw4SpStartup` roots = `maps/<mission>`, `codescripts/{delete,struct}`, and every `aitype/*` and
`animscripts/**` module the zones carry (`engine_named`). `Iw4SpStartup::load` drops an
engine-named root that does not compile (only the two stairs traverses). `animscript_module`
maps civilian states to `civilian/civilian_<state>` (grenade states → `civilian_grenade_response`,
others → `civilian_combat`).

Cost over 23 missions: roots 54–84 → 85–131, modules 4862 → 5155 (+7 to +25 per mission),
functions 131974 → 132862 (+0.7 %), compile ~150 ms per mission either way.
After: `gsc-audit all` → `no engine-named script missing`.
