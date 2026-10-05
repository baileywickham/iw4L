"""Generate a scripted run of so_takeover_estate (PMC elimination: 40 kills).

Run with IW4L_AUTOAIM=3 (hunt): while +attack is held the player aims at the
nearest hostile actor it sees, and when none is in sight it is moved to a path
node that sees the nearest one. The script touches the `mission_start` ring
(enemies populate only after it), then alternates bursts and reloads.

usage: gen.py [cycles] > run.cmds
"""
import sys

cycles = int(sys.argv[1]) if len(sys.argv) > 1 else 120
# `mission_start` is a ring of four hulls round the PMC start.
out = ['wait world', 'spawn 0', 'god', 'wait 3s', 'tp -239 -3190 -150 250 0', 'wait 5s', 'hold +speed_throw', 'screenshot start']
for i in range(cycles):
    out += ['hold +attack', 'wait 3s', 'release +attack', 'give ammo', 'press +reload', 'wait 1.5s']
    if i % 20 == 0:
        out.append(f'screenshot c{i}')
out += ['wait 8s', 'screenshot eog', 'wait 2s', 'finish_run']
print('; '.join(out))
