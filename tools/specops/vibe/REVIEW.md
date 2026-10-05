# Vibe check: capture, metrics, AI review

A playtester's "does this look right in motion" pass that an agent can run and read.

## Pipeline

```
capture.sh <mission> <cmds-file> <out-dir> [fps] [K=V ...]
  1. Air: map <mission> --cmds "<cmds with record vibe_<out> after spawn>; stoprecord; finish_run"
  2. Air: play vibe_<out> with IW4L_FRAME_DUMP=iw4l-artifacts/vibe/vibe_<out>,<fps>
  3. pull the dump back (tools/specops/bin/iw4l-air-pull)
  4. vibe.py all <out-dir>  ->  flags.json, summary.json, sheets/, capture.mp4/.gif, report.md
```

Example (what was run for the first review):

```
tools/specops/vibe/capture.sh so_intel_boneyard tools/specops/vibe/runs/intel_boneyard_60s.cmds context/vibe/intel 30
```

Reference footage (never committed; `context/` is ignored):

```
vibe.py ref https://www.youtube.com/watch?v=hIZ-zID1y8E 0 100 context/vibe-ref/snatch_grab
vibe.py compare context/vibe/intel context/vibe-ref/snatch_grab "60:3,300:12,900:30"
```

`compare` pairs are `ourFrame:refSeconds` (seconds into the downloaded segment). Pick them from
`ref_overview_*.png` (labels `rNNNN`, 0.5 s per step) and our `overview_*.png` (frame numbers).

## Engine side (dev only)

| env | meaning |
|---|---|
| `IW4L_FRAME_DUMP=<dir>[,fps]` | fixed simulated step `1/fps` (default 30) from launch (`TimeUpdateStrategy::ManualDuration`), every presented frame read back to `<dir>/NNNNN.png`; exits once the demo ends and the writes drain |
| `IW4L_FRAME_DUMP_FROM=replay\|live` | `replay` (default): only while a demo plays; `live`: whenever in game |
| `IW4L_FRAME_DUMP_WIDTH` | downscale width (default 960, `0` native) |
| `IW4L_FRAME_DUMP_MAX`, `IW4L_FRAME_DUMP_SKIP_MS` | frame cap; simulated ms to skip first |
| `IW4L_FRAME_DUMP_NO_PNG=1` | metrics only |

Next to the frames: `frames.csv` (sim ms, demo tick, wall ms, camera), `entities.csv` (per frame,
per animated script model / actor and per `ET_ITEM`: presented position and yaw from the drawn
transform, screen `u,v` of feet and head, on-screen/drawn, distance, weighted anim leaves, weight on
root-motion clips and their root speed, top clip, pose source, wrist span vs bind, mean bone offset
from bind pose, head height, ground gap from a static-world trace), `capture.json`.

The pose is re-derived the way `pose_script_models` does it (`resolve_request` → `pose_dobj` on a
DObj built from the same composition), so a T-pose the renderer draws is a T-pose the metrics see.

## Metric flags (`vibe.py analyze`)

| flag | rule (thresholds at the top of `vibe.py`) |
|---|---|
| `t_pose` | pose source is bind / resolve or pose failed, or animtree weight < 0.05, or mean bone offset from bind < 2.5 u |
| `running_in_place` | move-anim weight > 0.5 with root speed > 25 u/s, actual speed < 25% of it |
| `sliding_no_anim` | moving > 45 u/s with move-anim weight < 0.2 |
| `speed_mismatch` | moving with a move anim, actual / anim root speed outside 0.55–1.7 |
| `jitter` | moving; frame-to-frame step irregularity (mean abs step change / mean step) > 0.6, or > 20% near-zero steps (stepping) |
| `yaw_jitter` | mean abs yaw change per frame > 4° without a real turn |
| `floating_item` | `ET_ITEM` origin > 6 u above the ground trace |
| `actor_off_ground` | actor origin > 10 u above/below the ground trace |

Severity is downgraded to `low` when the entity was never on screen. Flags are leads, not verdicts:
the reviewer confirms each against the sheets.

## Review artifacts (`vibe.py sheets/video/report`)

- `sheets/overview_NN.png`: the whole capture in ≤ 8 4×4 sheets (evenly strided, frame numbers under each tile).
- `sheets/flagNN_<issue>_e<ent>_motion.png`: 16 frames (every 2nd) around the flag's closest on-screen frame.
- `sheets/flagNN_<issue>_e<ent>_crops.png`: the same frames cropped and zoomed on that entity.
- `sheets/compare.png`: ours | reference rows.
- `capture.mp4`, `capture.gif`; `report.md` ties flags and sheets together.

## Reviewer prompt (vision-capable subagent)

Give the agent the paths, not the images inline; it reads them with its file tool.

```
You are playtesting a reimplementation of Call of Duty: Modern Warfare 2 (2009) Spec Ops
mission <mission>. Judge it the way a player would, against your memory of the real game
and the reference frames provided.

Inputs (read every one):
- report:     <dump>/report.md  (metric flags: leads, not facts)
- overview:   <dump>/sheets/overview_*.png  (whole capture, frame numbers under tiles)
- flagged:    <dump>/sheets/flag*_motion.png and flag*_crops.png
- reference:  <ref>/ref_overview_*.png and <dump>/sheets/compare.png (ours left, MW2 right)

Rubric — for each category say what you see, with frame numbers:
1. Character pose: T-pose / bind pose, arms out, frozen mid-anim, limbs through bodies, wrong idle.
2. Locomotion: running in place, sliding/ice-skating, moon-walking, moving without legs moving,
   speed not matching stride.
3. Motion smoothness: jitter, stutter/stepping (position updates in steps), popping, snapping turns.
4. Grounding: characters or items floating, sunk into ground, weapons hovering.
5. Models: missing/invisible models, wrong/placeholder models, missing heads/weapons, z-fighting.
6. Lighting/materials: too dark/bright vs reference, missing textures (checker/pink/black), flat
   lighting, missing shadows/sky.
7. Effects: muzzle flash, tracers, impacts, smoke, explosions present and plausible.
8. HUD: compass, ammo, objective text, hit/damage indicator, crosshair; vs reference.
9. Camera/viewmodel: viewmodel clipping or misplaced, odd FOV, camera shake.

For each confirmed or suspected problem output one row:
| severity (blocker/major/minor/nit) | category | frames | entity (if any) | what you see | would a player notice? (yes/maybe/no) | confidence |

Then: metric flags you could NOT confirm visually (false positive or off-screen), anything
the metrics missed, and a one-paragraph verdict: "Would a player notice something is off in the
first minute? What is the single most jarring thing?"
Do not speculate about code; describe only what is visible.
```

Run it with the Agent tool (general-purpose agent, which can Read images) or do it inline by
reading the sheets yourself.
