"""so_download_arcadia: three laptop downloads, then extraction at the Stryker.

Teleport hops (<=450 u) along path-node routes, god on, and the `IW4L_AUTOFIRE=1` test aimer
(the defenders that charge each laptop must die or stay away, or the download is interrupted).
At a laptop the player presses +activate (the trigger_use starts the DSM download), then patrols
the path nodes 100-450 u around it (the aimer kills defenders in the house and the chargers), coming
back every ~10 s to press +activate again (resumes an interrupted transfer) and refill ammo. After the third
download the Stryker drives to `vnode_house1`; the player waits there until it is within 280 u.
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from so_lib import *

ents = load('download')
ns = nodes(ents)
laptops = {e['script_parameters']: e for e in by(ents, 'targetname', 'download')}
order = ['download_1_charger', 'download_2_charger', 'download_3_charger']
HOLD = int(os.environ.get('DL_HOLD', '130'))   # seconds spent at each laptop

cmds = ['wait world', 'spawn 0', 'god', 'wait 2s', tp([4743, -7889, 2400], 0), 'wait 1s',
        tp([4807, -7937, 2400], 0), 'wait 1s', 'screenshot dl_start']
at = [4807, -7937, 2400]
for i, name in enumerate(order):
    lap = laptops[name]
    o = lap['o']
    trig = by(ents, 'targetname', lap['target'])
    trig = [t for t in trig if t.get('classname') == 'trigger_use'][0]['o']
    stand = sorted(ns, key=lambda p: math.dist(p, trig) + abs(p[2] - trig[2]) * 2)[0]
    cmds += walk(route(ns, at, stand, hop=450)[1:-1], '1s')
    yaw, pitch = aim([stand[0], stand[1], stand[2] + 60], trig)
    cmds += [tp(stand, yaw, pitch), 'wait 1s', 'showpos', 'mark laptop%d' % (i + 1), 'press +activate', 'wait 2s',
             'screenshot dl_laptop%d' % (i + 1)]
    ring = [p for p in ns if 100 < math.dist(p[:2], trig[:2]) < 450 and abs(p[2] - stand[2]) < 50]
    ring.sort(key=lambda p: math.atan2(p[1] - trig[1], p[0] - trig[0]))
    t, k = 0.0, 0
    while t < HOLD:
        for p in ring[k % len(ring)::max(1, len(ring) // 6)][:6]:
            cmds += [tp(p, yaw_to(p, trig)), 'wait 1.5s']
            t += 1.6
        k += 1
        cmds += ['give ammo', tp(stand, yaw, pitch), 'wait 0.3s', 'press +activate', 'wait 0.3s', 'press +reload', 'wait 1s']
        t += 1.7
    at = stand
house1 = [5260, -8212, 2424]
cmds += walk(route(ns, at, house1, hop=450)[1:], '1s')
cmds += ['mark extraction', 'wait 25s', 'showpos', 'screenshot dl_end', 'wait 10s', 'screenshot dl_eog', 'finish_run']
emit(cmds)
