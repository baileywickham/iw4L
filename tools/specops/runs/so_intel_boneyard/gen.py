"""so_intel_boneyard: recover the intel at the laptop _pmc picked, then reach the extraction trigger.

`_pmc` picks one of the two `pmc_objective` laptops at random, so the run tries both (the other's
trigger_use is off). Teleport hops (<=450 u) along path-node routes, god on; at a laptop the player
holds +activate 4.5 s (3 s use bar, controls frozen meanwhile).
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from so_lib import *

ents = load('intel')
ns = nodes(ents)
laptops = by(ents, 'targetname', 'pmc_objective')
start = [-4704, -5616, -5]
mstart = [-5056, -5152, 16]
laptops.sort(key=lambda e: math.dist(e['o'], start))
ext = by(ents, 'targetname', 'me0_auto3')[0]['o']

cmds = ['wait world', 'spawn 0', 'god', 'wait 2s', tp(mstart, 45), 'wait 1s', 'screenshot intel_start']
at = mstart
for i, l in enumerate(laptops):
    o = l['o']
    stand = sorted(ns, key=lambda p: math.dist(p, o) + abs(p[2] - o[2]) * 2)[0]
    if math.dist(stand[:2], o[:2]) > 90:
        stand = [o[0] + 40, o[1], o[2]]
    cmds += walk(route(ns, at, stand, hop=450)[1:-1], '1s')
    yaw, pitch = aim([stand[0], stand[1], stand[2] + 60], o)
    cmds += [tp(stand, yaw, pitch), 'wait 1s', 'look %.1f %.1f' % (yaw, pitch), 'showpos', 'mark laptop%d' % i,
             'hold +activate', 'wait 4.5s', 'screenshot intel_laptop%d' % i, 'release +activate', 'wait 1.5s']
    at = stand
cmds += walk(route(ns, at, ext, hop=450)[1:], '1s')
cmds += [tp([ext[0], ext[1], ext[2] - 60]), 'wait 4s', 'showpos', 'screenshot intel_end', 'wait 8s', 'screenshot intel_eog', 'finish_run']
emit(cmds)
