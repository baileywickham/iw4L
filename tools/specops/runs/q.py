"""usage: q.py <ents name> <regex> [maxlen] — grep an entity dump (context/runs/ents_<name>.txt)."""
import os, re, sys
pat = re.compile(sys.argv[2])
n = int(sys.argv[3]) if len(sys.argv) > 3 else 260
for line in open(os.path.join(os.path.dirname(os.path.abspath(__file__)), 'ents_%s.txt' % sys.argv[1])):
    if pat.search(line):
        print(line.rstrip()[:n])
