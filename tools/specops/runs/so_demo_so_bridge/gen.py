"""so_demo_so_bridge: destroy all 36 target vehicles with the RPG.

Walks the bridge deck north (teleport hops over path nodes), god on. For each car still on the
list the player stands at the deck node closest to a point ~380 u south of it with no other car
on the line of fire, aims at the car's body and fires `rpg_player` twice (the second rocket hits
the wreck or finishes a car the first only set burning); `give ammo` (debug supply) refills the launcher between shots because
the mission's infinite-ammo bonus only runs for 30 s after every 5 wrecks. The cars are the
mission's own `_destructible` script models: rocket splash -> "damage" -> burn -> "exploded".
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from so_lib import *

ents = load('bridge')
ns = nodes(ents)
TARGETS = ('vehicle_undestroyed', 'missile_taxi', 'slide_car_1', 'slide_car_3', 'slide_car_4')
cars = [e for e in ents if e.get('classname') == 'script_model' and e.get('script_noteworthy') in TARGETS]
cars.sort(key=lambda e: e['o'][1])
BACK = float(os.environ.get('DEMO_BACK', '380'))
WAIT = os.environ.get('DEMO_WAIT', '3s')
SECOND = os.environ.get('DEMO_SECOND', '1') == '1'


def seg_dist(c, a, b):
    """Horizontal distance from point c to segment a-b."""
    ax, ay, bx, by_ = a[0], a[1], b[0], b[1]
    dx, dy = bx - ax, by_ - ay
    t = max(0.0, min(1.0, ((c[0] - ax) * dx + (c[1] - ay) * dy) / (dx * dx + dy * dy or 1)))
    return math.hypot(c[0] - ax - t * dx, c[1] - ay - t * dy)

cmds = ['wait world', 'spawn 0', 'god', 'wait 2s', 'screenshot demo_start']
at = [10720, 29952, -131]
for i, car in enumerate(cars):
    o = car['o']
    want = [o[0], o[1] - BACK, o[2]]
    deck = [p for p in ns if abs(p[2] - o[2]) < 60 and 200 < math.dist(p[:2], o[:2]) < 600 and p[1] > 29400]
    clear = [p for p in deck if all(seg_dist(c['o'], p, o) > 110 for c in cars if c is not car)]
    stand = nearest(clear or deck, want[0], want[1], want[2])
    hops = route(ns, at, stand, hop=450)[1:-1] if math.dist(at, stand) > 450 else []
    cmds += walk(hops, '0.6s')
    target = [o[0], o[1], o[2] + 28]
    yaw, pitch = aim([stand[0], stand[1], stand[2] + 60], target)
    cmds += [tp(stand, yaw, pitch), 'wait 0.6s', 'look %.1f %.1f &' % (yaw, pitch), 'wait 0.3s', 'mark car%02d' % i,
             'hold +attack', 'wait 0.3s', 'release +attack', 'wait %s' % WAIT, 'give ammo', 'press +reload', 'wait 0.5s']
    if SECOND:   # a second rocket: harmless on a wreck, finishes a car the first one only burned
        cmds += ['look %.1f %.1f &' % (yaw, pitch), 'wait 0.3s', 'hold +attack', 'wait 0.3s', 'release +attack',
                 'wait 2s', 'give ammo', 'press +reload', 'wait 0.5s']
    at = stand
cmds += ['wait 6s', 'showpos', 'screenshot demo_end', 'wait 10s', 'screenshot demo_eog', 'finish_run']
emit(cmds)
