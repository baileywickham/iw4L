"""so_crossing_so_bridge: cross the bridge deck from the start trigger to the `so_obj_crossing` trigger.

Teleport hops (~600 u) over path nodes on the deck, god on; the mission's own start trigger
(`so_obj_crossing_start`), bridge collapse trigger and end trigger fire from the player's touch.
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from so_lib import *

ents = load('bridge')
ns = nodes(ents)
end = [e for e in ents if e.get('script_noteworthy') == 'so_obj_crossing' and e.get('classname') == 'trigger_multiple'][0]

pts, z = [[10496, 32144, 6]], 10
for y in range(32700, 42900, 600):
    p = nearest(ns, 10496, y, z)
    z = p[2]
    pts.append(p)
pts.append(nearest(ns, 10496, 43104, z))

cmds = ['wait world', 'spawn 0', 'god', 'wait 2s', 'screenshot cross_start']
cmds += walk(pts, '1.5s')
cmds += ['showpos', 'wait 3s', 'screenshot cross_end', 'wait 8s', 'screenshot cross_eog', 'finish_run']
emit(cmds)
