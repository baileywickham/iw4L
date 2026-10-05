#!/usr/bin/env python3
"""Vibe-check tooling for an IW4L frame dump (IW4L_FRAME_DUMP).

    vibe.py analyze <dump>            entities.csv -> flags.json, summary.json
    vibe.py sheets  <dump>            contact sheets + per-flag crops (magick)
    vibe.py video   <dump>            capture.mp4 + capture.gif (ffmpeg)
    vibe.py report  <dump>            report.md (flags + thumbnails)
    vibe.py all     <dump>            all of the above
    vibe.py ref     <url> <start> <dur> <refdir>   fetch a reference clip (yt-dlp) + frames
    vibe.py compare <dump> <refdir> <pairs> [out]  side-by-side sheets, pairs = "f:sec,f:sec,..."

Stdlib only; ffmpeg/magick/yt-dlp from PATH (/opt/homebrew/bin on the Macs).
Thresholds are deliberately loose and named below so they can be tuned in one place.
"""
import csv
import json
import math
import os
import shutil
import subprocess
import sys
from collections import defaultdict

BIN = '/opt/homebrew/bin'
os.environ['PATH'] = BIN + ':' + os.environ.get('PATH', '')

# --- thresholds -------------------------------------------------------------
TPOSE_DEV = 2.5            # mean bone offset from bind pose (units) below which the body is "in bind pose"
TPOSE_WEIGHT = 0.05        # weighted (non-additive) anim leaves below this = nothing driving the skeleton
MOVING_SPEED = 25.0        # u/s horizontal: the entity counts as moving
RUN_IN_PLACE_FRAC = 0.25   # actual speed < this * anim root speed while a move anim dominates
SLIDE_NO_ANIM_SPEED = 45.0 # moving faster than this with (almost) no move anim weight
SPEED_MISMATCH = (0.55, 1.7)  # actual / anim root speed outside this = feet slide
JITTER_JERK = 0.6          # mean |d_i+1 - d_i| / mean d over a moving window (smooth ~0.1)
JITTER_STALL = 0.2         # share of near-zero steps between moving steps (stepping pattern)
YAW_JITTER_DEG = 4.0       # mean |second difference| of yaw per frame
FLOAT_ITEM = 6.0           # item origin above the ground (units)
FLOAT_ACTOR = 10.0         # actor origin above (or below, negated) the ground
WINDOW_S = 0.5
ACTOR_NOTICE_PX = 25       # an actor never taller than this on screen (render px) is 'low'
ITEM_NOTICE_DIST = 1500    # an item never closer than this is 'low'

FONT = next((p for p in ('/System/Library/Fonts/Supplemental/Arial.ttf', '/System/Library/Fonts/Menlo.ttc',
                         '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf') if os.path.exists(p)), 'DejaVu-Sans')


def f(v, default=math.nan):
    try:
        return float(v)
    except (TypeError, ValueError):
        return default


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, **kw)


def load(dump):
    frames = list(csv.DictReader(open(os.path.join(dump, 'frames.csv'))))
    ents = list(csv.DictReader(open(os.path.join(dump, 'entities.csv'))))
    cap = json.load(open(os.path.join(dump, 'capture.json'))) if os.path.exists(os.path.join(dump, 'capture.json')) else {}
    return frames, ents, cap


def ranges(frames_flagged, gap=3):
    """Group sorted frame numbers into [start, end] runs allowing small gaps."""
    out = []
    for fr in sorted(frames_flagged):
        if out and fr - out[-1][1] <= gap:
            out[-1][1] = fr
        else:
            out.append([fr, fr])
    return out


def analyze(dump):
    frames, ents, cap = load(dump)
    fps = float(cap.get('fps') or 30.0)
    dt = 1.0 / fps
    # Game time per presented frame: the demo clock, not the nominal step, if they disagree.
    if cap.get('first_tick') is not None and cap.get('last_tick') and cap.get('sim_ms'):
        rate = (cap['last_tick'] - cap['first_tick']) * 50.0 / cap['sim_ms']
        if 0.5 < rate < 2.0:
            dt *= rate
    H = float((cap.get('resolution') or [0, 720])[1] or 720)
    tracks = defaultdict(list)
    for r in ents:
        key = (r['kind'], r['entnum'] or r['model'])
        tracks[key].append(r)
    flags = []

    def add(issue, key, rows, frs, detail, severity):
        rowmap = {int(r['frame']): r for r in rows}
        for a, b in ranges(frs):
            span = [rowmap[x] for x in range(a, b + 1) if x in rowmap]
            vis = [r for r in span if r['on_screen'] == '1' and r['drawn'] == '1']
            dists = [f(r['dist']) for r in vis if not math.isnan(f(r['dist']))]
            if b - a + 1 < 3 and not vis:
                continue
            px = [abs(f(r['v']) - f(r['v_top'])) * H for r in vis if r['v'] and r['v_top']]
            big = max(px) if px else 0.0
            sev = severity
            if not vis or (key[0] == 'item' and min(dists or [1e9]) > ITEM_NOTICE_DIST) \
                    or (key[0] != 'item' and big < ACTOR_NOTICE_PX):
                sev = 'low'
            flags.append({
                'issue': issue, 'kind': key[0], 'entnum': key[1], 'model': span[0]['model'] if span else '',
                'frames': [a, b], 'n_frames': b - a + 1, 'on_screen_frames': len(vis),
                'min_dist': round(min(dists), 1) if dists else None,
                'max_px': round(big), 'severity': sev, 'detail': detail(span),
                'peak_frame': max(vis or span, key=lambda r: -f(r['dist'], 1e9))['frame'] if span else a,
            })

    for key, rows in tracks.items():
        rows.sort(key=lambda r: int(r['frame']))
        kind = key[0]
        if kind == 'item':
            fl = [int(r['frame']) for r in rows if f(r['ground_gap']) > FLOAT_ITEM]
            add('floating_item', key, rows, fl,
                lambda s: 'gap %.1f u above ground' % max(f(r['ground_gap'], 0) for r in s), 'high')
            continue
        # T-pose: explicit bind pose, failed anim resolve, nothing weighted, or posed ~= bind
        tp = []
        for r in rows:
            pose = r['pose']
            dev, w = f(r['pose_dev']), f(r['weight_sum'], 0)
            if pose in ('bind', 'resolve_err', 'pose_err') or (r['tree'] == '1' and w < TPOSE_WEIGHT) \
                    or (not math.isnan(dev) and dev < TPOSE_DEV):
                tp.append(int(r['frame']))
        add('t_pose', key, rows, tp, lambda s: 'pose=%s weight_sum=%s pose_dev=%s arm_span=%s/%s top=%s' % (
            s[0]['pose'], s[0]['weight_sum'], s[0]['pose_dev'], s[0]['arm_span'], s[0]['bind_span'], s[0]['top_clip']), 'high')
        # floating / sunk actor
        fl = [int(r['frame']) for r in rows if abs(f(r['ground_gap'], 0)) > FLOAT_ACTOR]
        add('actor_off_ground', key, rows, fl, lambda s: 'ground gap %.1f..%.1f u' % (
            min(f(r['ground_gap'], 0) for r in s), max(f(r['ground_gap'], 0) for r in s)), 'medium')
        # per-frame motion
        xs = [(int(r['frame']), f(r['x']), f(r['y']), f(r['z']), f(r['yaw'])) for r in rows]
        steps = {}
        for (fa, xa, ya, za, wa), (fb, xb, yb, zb, wb) in zip(xs, xs[1:]):
            if fb - fa != 1:
                continue
            d = math.hypot(xb - xa, yb - ya)
            if d > 200:  # teleport / respawn
                continue
            steps[fb] = (d, (wb - wa + 180) % 360 - 180)
        half = max(2, int(WINDOW_S * fps / 2))
        jit, slide, inplace, mismatch, yawj = [], [], [], [], []
        for r in rows:
            fr = int(r['frame'])
            win = [steps[x] for x in range(fr - half, fr + half + 1) if x in steps]
            if len(win) < half:
                continue
            ds = [d for d, _ in win]
            mean_d = sum(ds) / len(ds)
            speed = mean_d / dt
            move_w, anim_speed = f(r['move_w'], 0), f(r['anim_speed'], 0)
            r['_speed'] = speed
            if speed > MOVING_SPEED:
                jerk = sum(abs(b - a) for a, b in zip(ds, ds[1:])) / max(1, len(ds) - 1) / max(mean_d, 1e-3)
                stalls = sum(1 for d in ds if d < 0.15 * mean_d) / len(ds)
                r['_jerk'] = jerk
                if jerk > JITTER_JERK or stalls > JITTER_STALL:
                    jit.append(fr)
                if move_w < 0.2 and speed > SLIDE_NO_ANIM_SPEED:
                    slide.append(fr)
                elif move_w > 0.5 and anim_speed > MOVING_SPEED:
                    ratio = speed / anim_speed
                    if ratio < SPEED_MISMATCH[0] or ratio > SPEED_MISMATCH[1]:
                        mismatch.append(fr)
            elif move_w > 0.5 and anim_speed > MOVING_SPEED and speed < RUN_IN_PLACE_FRAC * anim_speed:
                inplace.append(fr)
            yaws = [w for _, w in win]
            if len(yaws) > 3:
                yj = sum(abs(b - a) for a, b in zip(yaws, yaws[1:])) / (len(yaws) - 1)
                if yj > YAW_JITTER_DEG and max(abs(y) for y in yaws) < 45:
                    yawj.append(fr)

        def motion(s):
            sp = [r.get('_speed', 0) for r in s]
            an = [f(r['anim_speed'], 0) for r in s]
            jk = [r['_jerk'] for r in s if '_jerk' in r]
            return 'speed %.0f u/s, anim root %.0f u/s, move_w %.2f, jerk %s, clip %s' % (
                sum(sp) / len(sp), sum(an) / len(an), sum(f(r['move_w'], 0) for r in s) / len(s),
                ('%.2f' % (sum(jk) / len(jk))) if jk else '-', s[len(s) // 2]['top_clip'])
        add('jitter', key, rows, jit, motion, 'medium')
        add('running_in_place', key, rows, inplace, motion, 'high')
        add('sliding_no_anim', key, rows, slide, motion, 'high')
        add('speed_mismatch', key, rows, mismatch, motion, 'medium')
        add('yaw_jitter', key, rows, yawj, motion, 'medium')

    wall = sorted(f(r['wall_ms']) for r in frames if f(r['wall_ms']) > 0)
    pct = lambda p: round(wall[min(len(wall) - 1, int(p * len(wall)))], 2) if wall else None
    actors = {k for k in tracks if k[0] == 'actor'}
    summary = {
        'dump': os.path.abspath(dump), 'zone': cap.get('zone'), 'fps': fps, 'frames': len(frames),
        'sim_seconds': round(len(frames) / fps, 2), 'game_seconds_per_frame': round(dt, 4), 'actors_seen': len(actors),
        'items_seen': len([k for k in tracks if k[0] == 'item']),
        'wall_ms_p50_p90_p99': [pct(0.5), pct(0.9), pct(0.99)],
        'flag_counts': {},
    }
    sev_rank = {'high': 0, 'medium': 1, 'low': 2}
    flags.sort(key=lambda x: (sev_rank[x['severity']], -x['on_screen_frames'], x['frames'][0]))
    for fl in flags:
        summary['flag_counts'][fl['issue']] = summary['flag_counts'].get(fl['issue'], 0) + 1
    json.dump(flags, open(os.path.join(dump, 'flags.json'), 'w'), indent=1)
    json.dump(summary, open(os.path.join(dump, 'summary.json'), 'w'), indent=1)
    print(json.dumps(summary, indent=1))
    return summary, flags


def frame_path(dump, n):
    return os.path.join(dump, '%05d.png' % n)


def montage(paths, out, tile='4x4', geom='480x+2+2', title=None):
    if not paths:
        return
    cmd = ['magick', 'montage', '-font', FONT, '-background', '#111', '-fill', 'white', '-pointsize', '14']
    if title:
        cmd += ['-title', title]
    cmd += ['-label', '%t'] + paths + ['-tile', tile, '-geometry', geom, out]
    run(cmd)


def sheets(dump, max_overview=8):
    frames, ents, cap = load(dump)
    flags = json.load(open(os.path.join(dump, 'flags.json'))) if os.path.exists(os.path.join(dump, 'flags.json')) else []
    out = os.path.join(dump, 'sheets')
    shutil.rmtree(out, ignore_errors=True)
    os.makedirs(os.path.join(out, 'crops'))
    nums = sorted(int(r['frame']) for r in frames if os.path.exists(frame_path(dump, int(r['frame']))))
    if not nums:
        print('no frames on disk')
        return
    fps = float(cap.get('fps') or 30)
    # overview: whole capture in <= max_overview 4x4 sheets
    stride = max(1, math.ceil(len(nums) / (16 * max_overview)))
    pick = nums[::stride]
    for i in range(0, len(pick), 16):
        chunk = pick[i:i + 16]
        montage([frame_path(dump, n) for n in chunk], os.path.join(out, 'overview_%02d.png' % (i // 16)),
                title='frames %d-%d (every %d = %.2fs)' % (chunk[0], chunk[-1], stride, stride / fps))
    # motion: consecutive frames (stride 2) around each flagged event, high/medium first
    rows_by = defaultdict(dict)
    for r in ents:
        rows_by[(r['kind'], r['entnum'] or r['model'])][int(r['frame'])] = r
    groups = {}
    for k, fl in enumerate(flags):
        if fl['severity'] == 'low':
            continue
        g = (fl['issue'], fl['model'])
        if g not in groups or fl['on_screen_frames'] > flags[groups[g]]['on_screen_frames']:
            groups[g] = k
    for k in sorted(groups.values())[:12]:
        fl = flags[k]
        centre = int(fl['peak_frame'])
        seq = [n for n in range(centre - 15, centre + 17, 2) if os.path.exists(frame_path(dump, n))]
        tag = 'flag%02d_%s_e%s' % (k, fl['issue'], fl['entnum'])
        montage([frame_path(dump, n) for n in seq], os.path.join(out, tag + '_motion.png'),
                title='%s ent %s %s frames %s' % (fl['issue'], fl['entnum'], fl['model'], fl['frames']))
        # zoomed crops of the entity over time
        track = rows_by[(fl['kind'], fl['entnum'])]
        crops = []
        for n in range(centre - 15, centre + 17, 2):
            r = track.get(n)
            src = frame_path(dump, n)
            if not r or not os.path.exists(src) or not r['u'] or not r['v_top']:
                continue
            box = crop_box(r, src)
            if not box:
                continue
            dst = os.path.join(out, 'crops', '%s_%05d.png' % (tag, n))
            w, h, x, y = box
            run(['magick', src, '-crop', '%dx%d+%d+%d' % (w, h, x, y), '+repage', '-resize', '240x240',
                 '-set', 'label', '%05d' % n, dst])
            crops.append(dst)
        if crops:
            run(['magick', 'montage', '-font', FONT, '-background', '#111', '-fill', 'white', '-pointsize', '13',
                 '-title', '%s ent %s (zoom)' % (fl['issue'], fl['entnum']), '-label', '%l'] + crops +
                ['-tile', '8x', '-geometry', '240x240+2+2', os.path.join(out, tag + '_crops.png')])
    print('sheets in', out)


_dims = {}


def image_size(path):
    d = os.path.dirname(path)
    if d not in _dims:
        w, h = subprocess.check_output(['magick', 'identify', '-format', '%w %h', path]).split()
        _dims[d] = (int(w), int(h))
    return _dims[d]


def crop_box(r, src):
    W, H = image_size(src)
    u, v, ut, vt = f(r['u']), f(r['v']), f(r['u_top']), f(r['v_top'])
    if any(math.isnan(x) for x in (u, v, ut, vt)):
        return None
    if r['kind'] == 'item':  # small model: wide box reaching below it, so the gap to the ground shows
        hpx = max(110, abs(v - vt) * H * 6)
        wpx = hpx * 1.3
        cx, cy = u * W, v * H + hpx * 0.2
    else:
        hpx = max(48, abs(v - vt) * H * 1.5)
        wpx = max(48, hpx * 0.9)
        cx, cy = u * W, (v + vt) / 2 * H
    x, y = int(max(0, min(W - wpx, cx - wpx / 2))), int(max(0, min(H - hpx, cy - hpx / 2)))
    if cx < -wpx or cx > W + wpx or cy < -hpx or cy > H + hpx:
        return None
    return int(min(wpx, W)), int(min(hpx, H)), x, y


def video(dump):
    _, _, cap = load(dump)
    fps = str(cap.get('fps') or 30)
    pat = os.path.join(dump, '%05d.png')
    run(['ffmpeg', '-loglevel', 'error', '-y', '-framerate', fps, '-i', pat, '-c:v', 'libx264', '-pix_fmt', 'yuv420p',
         '-crf', '23', '-vf', 'scale=trunc(iw/2)*2:trunc(ih/2)*2', os.path.join(dump, 'capture.mp4')])
    run(['ffmpeg', '-loglevel', 'error', '-y', '-i', os.path.join(dump, 'capture.mp4'), '-vf',
         'fps=12,scale=480:-1:flags=lanczos,split[a][b];[a]palettegen=max_colors=128[p];[b][p]paletteuse=dither=bayer',
         os.path.join(dump, 'capture.gif')])
    print('video:', os.path.join(dump, 'capture.mp4'), os.path.join(dump, 'capture.gif'))


def report(dump):
    summary = json.load(open(os.path.join(dump, 'summary.json')))
    flags = json.load(open(os.path.join(dump, 'flags.json')))
    sd = os.path.join(dump, 'sheets')
    L = ['# Vibe check: %s' % summary.get('zone'), '',
         '- frames: %d at %s fps (%.1f s simulated), actors seen %d, items seen %d' % (
             summary['frames'], summary['fps'], summary['sim_seconds'], summary['actors_seen'], summary['items_seen']),
         '- wall ms p50/p90/p99 (includes capture overhead): %s' % summary['wall_ms_p50_p90_p99'],
         '- flag counts: %s' % (', '.join('%s %d' % kv for kv in sorted(summary['flag_counts'].items())) or 'none'), '',
         '## Flags (metric-derived; verify against the sheets)', '',
         '| # | severity | issue | ent | model | frames | on-screen | min dist | max px | detail |', '|---|---|---|---|---|---|---|---|---|---|']
    for k, fl in enumerate(flags[:60]):
        L.append('| %d | %s | %s | %s | %s | %d-%d | %d | %s | %s | %s |' % (
            k, fl['severity'], fl['issue'], fl['entnum'], fl['model'], fl['frames'][0], fl['frames'][1],
            fl['on_screen_frames'], fl['min_dist'], fl.get('max_px'), fl['detail']))
    if len(flags) > 60:
        L.append('', )
        L.append('(%d more in flags.json)' % (len(flags) - 60))
    L += ['', '## Overview sheets', '']
    for name in sorted(os.listdir(sd)) if os.path.isdir(sd) else []:
        if name.startswith('overview_'):
            L.append('![%s](sheets/%s)' % (name, name))
    L += ['', '## Flagged events', '']
    for name in sorted(os.listdir(sd)) if os.path.isdir(sd) else []:
        if name.startswith('flag') and name.endswith('.png'):
            L.append('### %s' % name[:-4])
            L.append('![%s](sheets/%s)' % (name, name))
    for name in sorted(os.listdir(sd)) if os.path.isdir(sd) else []:
        if name.startswith('compare'):
            L.append('![%s](sheets/%s)' % (name, name))
    open(os.path.join(dump, 'report.md'), 'w').write('\n'.join(L) + '\n')
    print('report:', os.path.join(dump, 'report.md'))


def ref(url, start, dur, refdir):
    os.makedirs(refdir, exist_ok=True)
    clip = os.path.join(refdir, 'ref.mp4')
    if not os.path.exists(clip):
        end = float(start) + float(dur)
        run(['yt-dlp', '-q', '-f', 'bv*[height<=720][ext=mp4]/bv*[height<=720]/b', '--download-sections',
             '*%s-%s' % (start, end), '--force-keyframes-at-cuts', '-o', clip, url])
    fr = os.path.join(refdir, 'frames')
    shutil.rmtree(fr, ignore_errors=True)
    os.makedirs(fr)
    run(['ffmpeg', '-loglevel', 'error', '-i', clip, '-vf', 'fps=2,scale=960:-2', os.path.join(fr, 'r%04d.png')])
    json.dump({'url': url, 'start': float(start), 'dur': float(dur), 'frames_fps': 2},
              open(os.path.join(refdir, 'ref.json'), 'w'))
    n = len(os.listdir(fr))
    stride = max(1, math.ceil(n / 64))
    pick = sorted(os.listdir(fr))[::stride]
    for i in range(0, len(pick), 16):
        montage([os.path.join(fr, p) for p in pick[i:i + 16]], os.path.join(refdir, 'ref_overview_%02d.png' % (i // 16)),
                title='reference %s +%ss (r#### = 0.5 s steps)' % (url, start))
    print('reference frames:', fr)


def compare(dump, refdir, pairs, out=None):
    """pairs: 'ourFrame:refSeconds,...' (ref seconds relative to the downloaded clip start)."""
    out = out or os.path.join(dump, 'sheets', 'compare.png')
    os.makedirs(os.path.dirname(out), exist_ok=True)
    rows = []
    tmp = os.path.join(os.path.dirname(out), 'compare_rows')
    os.makedirs(tmp, exist_ok=True)
    for i, pr in enumerate(pairs.split(',')):
        ours, sec = pr.split(':')
        ours = int(ours)
        rf = os.path.join(refdir, 'frames', 'r%04d.png' % (int(float(sec) * 2) + 1))
        of = frame_path(dump, ours)
        if not (os.path.exists(rf) and os.path.exists(of)):
            print('skip pair', pr)
            continue
        row = os.path.join(tmp, 'row%02d.png' % i)
        run(['magick', '(', of, '-resize', '640x', '-gravity', 'north', '-font', FONT, '-background', '#111', '-fill', 'white',
             '-pointsize', '16', '-splice', '0x22', '-annotate', '+0+3', 'IW4L frame %d' % ours, ')',
             '(', rf, '-resize', '640x', '-gravity', 'north', '-font', FONT, '-background', '#111', '-fill', 'white',
             '-pointsize', '16', '-splice', '0x22', '-annotate', '+0+3', 'MW2 reference +%ss' % sec, ')',
             '+append', row])
        rows.append(row)
    if rows:
        run(['magick'] + rows + ['-append', out])
        print('compare:', out)


if __name__ == '__main__':
    a = sys.argv[1:]
    if not a:
        print(__doc__)
        sys.exit(1)
    cmd = a[0]
    if cmd == 'analyze':
        analyze(a[1])
    elif cmd == 'sheets':
        sheets(a[1])
    elif cmd == 'video':
        video(a[1])
    elif cmd == 'report':
        report(a[1])
    elif cmd == 'all':
        analyze(a[1])
        sheets(a[1])
        video(a[1])
        report(a[1])
    elif cmd == 'ref':
        ref(a[1], a[2], a[3], a[4])
    elif cmd == 'compare':
        compare(a[1], a[2], a[3], a[4] if len(a) > 4 else None)
    else:
        print(__doc__)
        sys.exit(1)
