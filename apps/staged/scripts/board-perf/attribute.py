#!/usr/bin/env python3
"""Attribute a CPU profile from `run.mjs --profile` to app-level work.

Every sample is put in a phase: under a store write (Svelte marking the
readers of a changed signal dirty), under the effect flush that follows, or
elsewhere. Within a phase, time is totalled per deepest frame that is not
Svelte runtime code, so the output names the derived values and effects that
re-ran. "(runtime only)" is the runtime walking its own graph.

Runtime code is taken to be the first RUNTIME_END lines of the index bundle
of an unminified build (`vite build --minify false`); check where the
`svelte/internal` functions end in yours and pass it if it differs.

Usage: attribute.py <file.cpuprofile> [runtime_end_line]"""
import json, sys, collections
prof = json.load(open(sys.argv[1]))
RUNTIME_END = int(sys.argv[2]) if len(sys.argv) > 2 else 6500
nodes = {n['id']: n for n in prof['nodes']}
parent = {}
for n in prof['nodes']:
    for c in n.get('children', []):
        parent[c] = n['id']
def frames(nid):
    out = []
    while nid is not None:
        out.append(nodes[nid]['callFrame'])
        nid = parent.get(nid)
    return out  # leaf first
def is_runtime(f):
    return 'index-' in f['url'] and f['lineNumber'] + 1 < RUNTIME_END
def name(f):
    return f"{f['functionName'] or '(anonymous)'}:{f['lineNumber'] + 1}"
phase = collections.Counter(); by_app = collections.Counter(); total = 0
cache = {}
for nid, dt in zip(prof['samples'], prof['timeDeltas']):
    total += dt
    if nid not in cache:
        fs = frames(nid)
        names = [f['functionName'] for f in fs]
        leaf = fs[0]
        if leaf['functionName'] in ('(idle)',):
            ph = 'idle'
        elif leaf['functionName'] in ('(program)', '(garbage collector)'):
            ph = 'browser (program, GC)'
        elif 'flush' in names or '#process' in names:
            ph = 'effect flush'
        elif 'mark_reactions' in names or 'internal_set' in names:
            ph = 'store write (mark dirty)'
        else:
            ph = 'other script'
        # The deepest frame outside the Svelte runtime: the app code at work.
        app = next((name(f) for f in fs if f['url'] and not is_runtime(f)), '(runtime only)')
        cache[nid] = (ph, app)
    ph, app = cache[nid]
    phase[ph] += dt
    by_app[(ph, app)] += dt
print(f'total {total/1000:.0f} ms')
for ph, us in phase.most_common():
    print(f'\n== {ph}: {us/1000:.0f} ms ({us/total*100:.1f}%)')
    rows = [(a, u) for (p, a), u in by_app.items() if p == ph]
    for a, u in sorted(rows, key=lambda r: -r[1])[:14]:
        print(f'   {u/1000:8.0f} ms  {a}')
