"""Generate a scripted full run of The Pit (so_killspree_trainer).

Reads target/rail/trigger data from ents.txt (DEBUGENT dump of the map + mission
entities) and prints a `--cmds` script on stdout (the plan goes to stderr).

Boards on the player's floor are shot from close range (40-90 units in front of
the board) at their lower half, so a round that misses or passes a dropped board
hits the floor right behind it instead of a civilian further back. Rail boards
stop sliding once the player is within 128 units (trainer.gsc
delete_when_player_too_close), so their vantage sits close to the rail and the
whole rail is swept once. Boards on the upper floor of the first area are shot
from the corridor below. Every candidate line of fire is checked against the
civilian boards (and their rails) already standing.

PIT_SHOTS=1 adds a screenshot per vantage.
"""
import math
import os
import sys

os.chdir(os.path.dirname(os.path.abspath(__file__)))

EYE = 60.0
SHOTS = os.environ.get('PIT_SHOTS') == '1'
ORDER = [1, 2, 3, 4, 5, 14, 6, 12, 13]  # so_killspree_trainer course_trigger_sort
GROUND = -190.0

ents, seen = [], set()
for line in open('ents.txt'):
    line = line.strip()[len('DEBUGENT '):]
    if line in seen:
        continue
    seen.add(line)
    d = {}
    for kv in line.split(' | '):
        if '=' in kv:
            k, v = kv.split('=', 1)
            d[k.lower()] = v
    ents.append(d)
byname = {}
for e in ents:
    if e.get('targetname'):
        byname.setdefault(e['targetname'], e)
vec = lambda s: [float(x) for x in s.split()]
rails = {e['origin']: e for e in ents if e.get('targetname') == 'target_rail_start_point'}


class Board:
    def __init__(self, e):
        self.stage = int(e['script_linkname'])
        self.enemy = e['script_noteworthy'] == 'target_enemy'
        self.melee = e.get('script_parameters') == 'melee'
        org = byname[e['target']]
        self.base = vec(org['origin'])
        aim = vec(byname[org['target']]['origin'])
        self.off = [aim[i] - self.base[i] for i in range(3)]
        self.facing = float(org.get('angles', '0 0 0').split()[1])
        start = rails.get(org['origin'])
        self.rail = None
        if e.get('script_parameters') == 'use_rail' and start:
            self.rail = (vec(start['origin']), vec(byname[start['target']]['origin']))

    def points(self, step=12.0, dz=None):
        off = list(self.off)
        if dz is not None:
            off[2] = dz
        if not self.rail:
            return [[self.base[i] + off[i] for i in range(3)]]
        a, b = self.rail
        n = max(1, int(math.dist(a, b) / step))
        return [[a[i] + (b[i] - a[i]) * k / n + off[i] for i in range(3)] for k in range(n + 1)]

    def mid(self):
        p = self.points()
        return p[len(p) // 2]


boards = [Board(e) for e in ents if e.get('classname') == 'script_brushmodel'
          and e.get('script_noteworthy') in ('target_enemy', 'target_friendly')
          and e.get('script_linkname', '').isdigit() and int(e['script_linkname']) in ORDER]
civs = [b for b in boards if not b.enemy]


def seg_dist(p, a, b):
    ab = [b[i] - a[i] for i in range(3)]
    ap = [p[i] - a[i] for i in range(3)]
    l2 = sum(x * x for x in ab) or 1.0
    t = max(0.0, min(1.0, sum(ap[i] * ab[i] for i in range(3)) / l2))
    return math.dist(p, [a[i] + ab[i] * t for i in range(3)])


def ray_end(eye, aim, floor):
    """Where the round stops: the floor plane when aimed down, else 600 units past the aim."""
    d = [aim[i] - eye[i] for i in range(3)]
    n = math.hypot(*d) or 1.0
    if d[2] < -1e-3:
        t = (floor - eye[2]) / d[2]
        if t > 1.0:
            return [eye[i] + d[i] * t for i in range(3)]
    return [eye[i] + d[i] / n * (n + 600) for i in range(3)]


def civ_clearance(eye, aim, floor, stage):
    end = ray_end(eye, aim, floor)
    best = 1e9
    for c in civs:
        if ORDER.index(c.stage) > ORDER.index(stage):
            continue
        for p in c.points(24):
            for dz in (-35, 0, 25):
                best = min(best, seg_dist([p[0], p[1], p[2] + dz], eye, end))
    return best


def angles(eye, p):
    dx, dy, dz = p[0] - eye[0], p[1] - eye[1], p[2] - eye[2]
    return math.degrees(math.atan2(dy, dx)), -math.degrees(math.atan2(dz, math.hypot(dx, dy)))


def near_board(v, b):
    for o in boards:
        if ORDER.index(o.stage) > ORDER.index(b.stage) or abs(o.base[2] - v[2]) > 60:
            continue
        for p in o.points(12):
            if math.hypot(p[0] - v[0], p[1] - v[1]) < 35:
                return True
    return False


def face_on(v, b, pts):
    """Every aim point is seen within 75° of the board's normal (either side): no edge-on shots."""
    n = (math.cos(math.radians(b.facing)), math.sin(math.radians(b.facing)))
    for p in pts:
        d = (v[0] - p[0], v[1] - p[1])
        if abs(d[0] * n[0] + d[1] * n[1]) < math.hypot(*d) * math.cos(math.radians(75)):
            return False
    return True


# Corridor points for the upper-floor boards of the first area, seen from below.
corridor = [(x, 2232, GROUND) for x in range(-4800, -5300, -40)]


def plan(b):
    """(vantage feet origin, aim points, civilian clearance)."""
    if b.stage in (3, 4) and b.base[2] > -100:
        best = None
        for v in corridor:
            eye = [v[0], v[1], v[2] + EYE]
            pts = b.points()
            h = math.hypot(b.mid()[0] - v[0], b.mid()[1] - v[1])
            clear = min(civ_clearance(eye, p, GROUND, b.stage) for p in pts)
            score = abs(h - 220) + (0 if clear > 60 else 5000)
            if best is None or score < best[0]:
                best = (score, v, pts, clear)
        return best[1], best[2], best[3], False
    floor = b.base[2]
    m = b.mid()
    best = None
    # Centre aim first. Low aim keeps misses off civilians further back (rounds hit the floor
    # just behind the board) but clips the low walls some boards stand behind: fallback only.
    for low, need in ((False, 45), (True, 45)):
        pts = b.points(step=16, dz=30 if low else None)
        for r in range(40, 200, 5):
            for da in range(-180, 180, 5):
                a = math.radians(b.facing + da)
                v = (round(m[0] + r * math.cos(a)), round(m[1] + r * math.sin(a)), floor + 2)
                if near_board(v, b) or not face_on(v, b, pts):
                    continue
                eye = [v[0], v[1], v[2] + EYE]
                clear = min(civ_clearance(eye, p, floor, b.stage) for p in pts)
                pref = 45 if b.rail else 80
                score = abs(r - pref) + min(abs(da), 60) * 1.0 + max(0, abs(da) - 60) * 0.3 + (0 if clear > need else 3000 + (need - clear) * 40)
                if best is None or score < best[0]:
                    best = (score, v, pts, clear, low)
        if best[0] < 3000:
            break
    return best[1], best[2], best[3], best[4]


cmds = ['wait world', 'spawn 0', 'wait 2s', 'god', 'hold +speed_throw']
fired = 0
report = []


def tp(v, yaw=0.0, pitch=0.0, t='0.4s'):
    cmds.append(f'tp {v[0]:.0f} {v[1]:.0f} {v[2]:.0f} {yaw:.1f} {pitch:.1f} &')
    cmds.append(f'wait {t}')


def shot(eye, p):
    """`hold +attack` needs ≥0.2 s to fire through the input path."""
    global fired
    yaw, pitch = angles(eye, p)
    cmds.extend([f'look {yaw:.1f} {pitch:.1f} &', 'wait 0.12s', 'hold +attack', 'wait 0.25s',
                 'release +attack', 'wait 0.05s'])
    fired += 1
    if fired % 7 == 0:  # a 0.25 s hold fires 3-4 rounds of a 30-round clip
        cmds.extend(['give ammo', 'press +reload', 'wait 2.6s'])


def stage(n):
    cmds.append(f'mark stage{n}')
    for b in [b for b in boards if b.stage == n and b.enemy]:
        v, pts, clear, low = plan(b)
        report.append(f'stage {n:2} board {b.base} rail={bool(b.rail)} vantage={v} clear={clear:.0f} low={low}')
        eye = [v[0], v[1], v[2] + EYE]
        tp(v, *angles(eye, pts[0]))
        if SHOTS:
            cmds.append(f'screenshot st{n}_{len(report)}')
        for p in pts:
            shot(eye, p)
    cmds.append('wait 0.6s')


tp((-3690, 2160, GROUND), 180, 0, '1.5s')   # target_trigger 1: course start
for n in (1, 2, 3, 4, 5):
    stage(n)
# Stairs: so_player_melee_trigger sets player_course_stairs2; the sideways board (stage 14)
# only pops while the player stays below y 2520, and only MOD_MELEE counts.
tp((-5360, 2232, -150), 180, 0, '0.6s')     # player_course_03a (so_course_loop_think waits on it)
tp((-5729, 2397, -128), 90, 0, '0.6s')      # player_course_stairs
tp((-5730, 2474, -40), 90, 0, '0.8s')       # so_player_melee_trigger
cmds += ['mark stage14', 'tp -5723 2498 -40 90 2 &', 'wait 1s', 'showpos']
if SHOTS:
    cmds.append('screenshot melee')
for pitch in (2, 10, -6, 2):
    cmds += [f'look 90 {pitch}', 'press +melee', 'wait 0.9s']
tp((-5728, 2519, -8), 0, 0, '0.8s')         # player_course_upstairs
stage(6)
tp((-5391, 2777, 0), 0, 0, '0.6s')          # player_course_jumping_down
tp((-5175, 2777, -124), 0, 0, '0.8s')       # player_course_jumped_down
tp((-4420, 2812, -148), 0, 0, '1s')         # so_player_course_jumped_down (needs half_way)
stage(12)
stage(13)
cmds += ['wait 1.5s', 'screenshot so_pre_end']
tp((-3586, 2881, -136), 0, 0, '8s')         # end_trigger: so_player_course_end
cmds += ['screenshot so_end', 'wait 6s', 'screenshot so_eog', 'wait 1s', 'finish_run']
print('; '.join(cmds))
for r in report:
    print(r, file=sys.stderr)
print(len(cmds), 'commands', fired, 'shots', file=sys.stderr)
