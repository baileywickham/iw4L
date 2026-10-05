"""so_escape_airport: run the terminal from the start trigger to `escaped_trigger`.

Teleport hops (<=450 u) along a path-node route through the mission's own triggers in story
order (start_terminal, waiting area, shopping, dining, stores, escalators, security, exit); god on.
"""
import sys, os
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), '..'))
from so_lib import *

ents = load('escape')
ns = nodes(ents)

WAY = [
    (3134, 3326, 70),    # start_terminal
    (2391, 3661, 70),    # enemy_waiting_area_above_movein_trig
    (2120, 4232, 240),   # obj_shopping
    (2604, 4768, 370),   # shoot_out_glass
    (2856, 5132, 370),   # enemy_dining_area_riot_movein_trig
    (3520, 3806, 370),   # enemy_store_area_start_movein_trig
    (3488, 4408, 370),   # stop_board_flipping
    (3488, 4168, 370),   # obj_escalators_end
    (4318, 2606, 370),   # *_kill flag triggers
    (4709, 2501, 370),   # obj_finish
    (4780, 2300, 370),   # enemy_security_area_final_movein_trig
    (6740, 1264, 190),   # escaped_trigger
]

pts = [WAY[0]]
for a, b in zip(WAY, WAY[1:]):
    pts += route(ns, a, b, hop=450)[1:]
    pts.append(b)

cmds = ['wait world', 'spawn 0', 'god', 'wait 2s', 'screenshot esc_start']
cmds += walk(pts, '1.2s')
cmds += ['showpos', 'wait 3s', 'screenshot esc_end', 'wait 8s', 'screenshot esc_eog', 'finish_run']
emit(cmds)
