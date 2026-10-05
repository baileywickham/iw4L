import os, sys
os.chdir(os.path.dirname(os.path.abspath(__file__)))
ents = []
seen = set()
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

cmds = ['wait world', 'spawn 0', 'wait 2s', 'god', 'hold +speed_throw',
        'tp -3690 2170 -150 180 0 &', 'wait 5s']
shots = 0

def fire(x, y, z, yaw, pitch=18):
    cmds.append(f'tp {x:.0f} {y:.0f} {z + 2:.0f} {yaw} {pitch:.0f} &')
    cmds.append('wait 500ms')
    cmds.append('hold +attack')
    cmds.append('wait 200ms')
    cmds.append('release +attack')
    cmds.append('wait 150ms')

def reload_maybe():
    global shots
    shots += 1
    if shots % 3 == 0:
        cmds.append('press +reload')
        cmds.append('wait 2500ms')

def shoot_at(x, y, z):
    for yaw, dx, dy in [(180, 70, 0), (0, -70, 0), (270, 0, 70), (90, 0, -70)]:
        fire(x + dx, y + dy, z, yaw)
    reload_maybe()

def shoot_rail(a, b, off):
    import math
    dx, dy = b[0] - a[0], b[1] - a[1]
    n = math.hypot(dx, dy) or 1
    px, py = -dy / n, dx / n
    yaw = math.degrees(math.atan2(-py, -px))
    for k in range(5):
        t = k / 4
        x = a[0] + dx * t + off[0]
        y = a[1] + dy * t + off[1]
        for yaw, ox, oy in [(180, 70, 0), (0, -70, 0), (270, 0, 70), (90, 0, -70)]:
            fire(x + ox, y + oy, a[2], yaw)
        reload_maybe()

def stage(n):
    for e in ents:
        if e.get('script_linkname') == str(n) and e.get('classname') == 'script_brushmodel' \
                and e.get('script_noteworthy') == 'target_enemy':
            if e.get('script_parameters') == 'melee':
                continue
            o = vec(e['origin'])
            org = byname[e['target']]
            aim = vec(byname[org['target']]['origin'])
            off = [aim[i] - o[i] for i in range(3)]
            start = rails.get(org['origin'])
            if e.get('script_parameters') == 'use_rail' and start:
                shoot_rail(vec(start['origin']), vec(byname[start['target']]['origin']), off)
            else:
                shoot_at(o[0] + off[0], o[1] + off[1], o[2])
    cmds.append('wait 1500ms')

def touch(x, y, z, t='1s'):
    cmds.append(f'tp {x} {y} {z} 0 0 &')
    cmds.append(f'wait {t}')

for n in [1, 2, 3, 4, 5]:
    stage(n)
touch(-5360, 2232, -136)          # player_course_03a
touch(-5730, 2474, -36)           # stairs2 / so melee trigger
cmds += ['release +speed_throw', 'tp -5715 2505 -40 90 0 &', 'wait 500ms', 'press +melee', 'wait 800ms',
         'press +melee', 'wait 800ms', 'tp -5715 2505 -10 90 20 &', 'wait 300ms', 'press +melee', 'wait 1500ms',
         'hold +speed_throw']
touch(-5728, 2519, -8)            # upstairs
stage(6)
touch(-5391, 2777, 0)             # jumping_down
touch(-5175, 2777, -124)
touch(-4420, 2812, -148, '1500ms')  # so jumped_down (after half_way)
stage(12)
stage(13)
cmds += ['wait 2s', 'screenshot so_pre_end']
touch(-3586, 2881, -136, '8s')    # end trigger
cmds += ['screenshot so_end', 'wait 6s', 'screenshot so_eog', 'wait 1s', 'finish_run']
print('; '.join(cmds))
print(len(cmds), file=sys.stderr)
