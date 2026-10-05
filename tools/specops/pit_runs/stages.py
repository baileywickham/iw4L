import os
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
trig = [e for e in ents if e.get('targetname') == 'target_trigger']
byname = {}
for e in ents:
    if e.get('targetname'):
        byname.setdefault(e['targetname'], e)
for t in sorted(trig, key=lambda t: int(t['script_linkto'])):
    n = t['script_linkto']
    print('stage', n, t['origin'], t.get('script_noteworthy'))
    for e in ents:
        if e.get('script_linkname') == n and e.get('classname') == 'script_brushmodel' and e.get('script_noteworthy') in ('target_enemy', 'target_friendly'):
            org = byname.get(e.get('target'), {})
            print('   ', e['script_noteworthy'][7:], e['origin'], e.get('script_parameters'), org.get('script_noteworthy'), org.get('angles'), org.get('origin'))
for e in ents:
    if 'rail' in e.get('targetname', ''):
        print('rail', e)
