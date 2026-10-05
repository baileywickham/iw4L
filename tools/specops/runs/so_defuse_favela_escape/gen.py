"""so_defuse_favela_escape: defuse the three briefcase bombs (market, apartment, store).

Teleport hops (<=450 u) along path-node routes, god on. At each bomb the player stands at the
nearest path node, looks at the briefcase and holds +activate 6.5 s: the use fires the briefcase's
"trigger", the script links the player, switches to `briefcase_bomb_defuse_sp` ("weapon_change")
and runs its 4.5 s UseButtonPressed bar.
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from so_lib import *

ents = load('defuse')
ns = nodes(ents)
bombs = sorted(by(ents, 'targetname', 'defuse_briefcase'), key=lambda e: int(e['script_index']))

start = [-848, 2048, 1104]
cmds = ['wait world', 'spawn 0', 'god', 'wait 2s', tp([-796, 1968, 1110], -90), 'wait 1s', 'screenshot def_start']
at = [-796, 1968, 1110]
for b in bombs:
    o = b['o']
    stand = sorted(ns, key=lambda p: math.dist(p, o) + abs(p[2] - o[2]) * 2)[0]
    pts = route(ns, at, stand, hop=450)[1:-1]
    cmds += walk(pts, '1s')
    yaw, pitch = aim([stand[0], stand[1], stand[2] + 60], o)
    cmds += [tp(stand, yaw, pitch), 'wait 1s', 'look %.1f %.1f' % (yaw, pitch), 'showpos', 'mark bomb%s' % b['script_index'],
             'hold +activate', 'wait 6.5s', 'screenshot def_bomb%s' % b['script_index'], 'release +activate', 'wait 2.5s']
    at = stand
cmds += ['wait 3s', 'screenshot def_end', 'wait 8s', 'screenshot def_eog', 'finish_run']
emit(cmds)
