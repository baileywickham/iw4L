"""Dump the entity strings of MW2 zones as one `key=value | ...` line per entity.

usage: ents.py <out.txt> <zone>...   (zone names, from $IW4L_GAMES/MW2/zone/english)
Base map + mission zone together give what a Spec Ops load spawns (mission AddonMapEnts last).
"""
import os, re, sys, zlib

root = os.path.join(os.environ.get('IW4L_GAMES', os.path.expanduser('~/Games')), 'MW2/zone/english')
out = open(sys.argv[1], 'w')
for z in sys.argv[2:]:
    b = open(os.path.join(root, z + '.ff'), 'rb').read()
    i = b.find(b'\x78\xda', 0, 64)
    data = zlib.decompressobj().decompress(b[i:])
    n = 0
    for m in re.finditer(rb'\{\n(?:"[^"\n]*" "[^"\n]*"\n)+\}', data):
        kv = re.findall(rb'"([^"\n]*)" "([^"\n]*)"', m.group(0))
        out.write('zone=%s | ' % z + ' | '.join('%s=%s' % (k.decode(errors='replace'), v.decode(errors='replace')) for k, v in kv) + '\n')
        n += 1
    print(z, n, file=sys.stderr)
