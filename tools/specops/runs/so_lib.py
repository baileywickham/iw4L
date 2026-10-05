"""Shared helpers for the scripted Spec Ops runs (entity dumps from ents.py -> console commands)."""
import math
import os

HERE = os.path.dirname(os.path.abspath(__file__))


def vec(s):
    return [float(x) for x in s.split()]


def load(name):
    """Entities of context/runs/ents_<name>.txt as dicts (keys lowercased)."""
    ents = []
    for line in open(os.path.join(HERE, 'ents_%s.txt' % name)):
        d = {}
        for kv in line.rstrip('\n').split(' | '):
            if '=' in kv:
                k, v = kv.split('=', 1)
                d[k.lower()] = v
        if 'origin' in d:
            d['o'] = vec(d['origin'])
        ents.append(d)
    return ents


def by(ents, key, value):
    return [e for e in ents if e.get(key) == value]


def nodes(ents):
    return [e['o'] for e in ents if e.get('classname', '').startswith('node_') and 'o' in e]


def nearest(pts, x, y, z=None):
    def d(p):
        dz = 0 if z is None else (p[2] - z) * 4
        return (p[0] - x) ** 2 + (p[1] - y) ** 2 + dz * dz
    return min(pts, key=d)


def yaw_to(a, b):
    return math.degrees(math.atan2(b[1] - a[1], b[0] - a[0]))


def aim(eye, target):
    """yaw, pitch (positive = down) from eye to target."""
    dx, dy, dz = target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]
    yaw = math.degrees(math.atan2(dy, dx))
    pitch = -math.degrees(math.atan2(dz, math.hypot(dx, dy)))
    return yaw, pitch


def tp(p, yaw=0.0, pitch=0.0, dz=4):
    return 'tp %d %d %d %.1f %.1f &' % (round(p[0]), round(p[1]), round(p[2] + dz), yaw, pitch)


def walk(pts, step_wait='1s'):
    """tp through a list of points (each snapped by the caller), facing the next one."""
    out = []
    for i, p in enumerate(pts):
        nxt = pts[i + 1] if i + 1 < len(pts) else None
        yaw = yaw_to(p, nxt) if nxt else 0.0
        out.append(tp(p, yaw))
        out.append('wait %s' % step_wait)
    return out


def emit(cmds):
    print('; '.join(cmds))


def route(ns, a, b, link=320.0, max_dz=48.0, hop=500.0):
    """Approximate walkable route between points a and b over path nodes (Dijkstra; nodes link
    within `link` u horizontally and a slope-limited height), thinned to hops of <= `hop` u."""
    import heapq
    cell = {}
    for i, p in enumerate(ns):
        cell.setdefault((int(p[0] // link), int(p[1] // link)), []).append(i)

    def near(i):
        p = ns[i]
        cx, cy = int(p[0] // link), int(p[1] // link)
        for dx in (-1, 0, 1):
            for dy in (-1, 0, 1):
                for j in cell.get((cx + dx, cy + dy), ()):
                    q = ns[j]
                    h = math.hypot(q[0] - p[0], q[1] - p[1])
                    if j != i and h <= link and abs(q[2] - p[2]) <= max_dz + h * 0.6:
                        yield j, math.hypot(h, q[2] - p[2])
    s = ns.index(nearest(ns, a[0], a[1], a[2]))
    t = ns.index(nearest(ns, b[0], b[1], b[2]))
    dist, prev, pq = {s: 0.0}, {}, [(0.0, s)]
    while pq:
        d, i = heapq.heappop(pq)
        if i == t:
            break
        if d > dist.get(i, 1e18):
            continue
        for j, w in near(i):
            if d + w < dist.get(j, 1e18):
                dist[j], prev[j] = d + w, i
                heapq.heappush(pq, (d + w, j))
    if t not in dist:
        raise SystemExit('no route %s -> %s' % (a, b))
    path = [t]
    while path[-1] != s:
        path.append(prev[path[-1]])
    path = [ns[i] for i in reversed(path)]
    out = [path[0]]
    for k, p in enumerate(path[1:], 1):
        nxt = path[k + 1] if k + 1 < len(path) else None
        if nxt is None or math.dist(out[-1], nxt) > hop:
            out.append(p)
    return out
