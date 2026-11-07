#!/usr/bin/python3 -I
"""fill-unassigned.py <old-map-dir>: assign map/unassigned-new.tsv lines after remap-map.py.

A line takes the owner every old copy of the same (sign, text) had, when that owner is unique;
otherwise (braces, blanks) the owner of its nearest assigned neighbour in the same hunk. Rewrites the
rows of each affected diff as runs, and prints every neighbour-based choice for review."""
import collections, glob, sys

M = '/mnt/logs/temp/history-rebuild/map'
OLD = sys.argv[1]


def body(text):
    out, h, k = [], 0, 0
    for ln in text.split('\n'):
        if ln.startswith('@@'):
            h += 1; k = 0
        elif h and ln and ln[0] in '+-\\':
            k += 1; out.append((h, k, ln[0], ln[1:]))
    return out


def owners(mapdir, name):
    own, files = {}, set()
    for f in glob.glob(mapdir + '/assign/*.tsv'):
        if f.endswith('.deps.tsv'):
            continue
        for l in open(f):
            p = l.rstrip('\n').split('\t')
            if len(p) >= 5 and p[0] == name and p[1].isdigit():
                files.add(f)
                a, b = map(int, p[2].split('-'))
                for k in range(a, b + 1):
                    own[(int(p[1]), k)] = (p[3], p[4], p[5] if len(p) > 5 else '')
    return own, files


rows = [l.rstrip('\n').split('\t') for l in open(M + '/unassigned-new.tsv') if l.strip()]
for name in sorted({r[0] for r in rows}):
    old_own, _ = owners(OLD, name)
    cand = collections.defaultdict(set)
    for h, k, s, t in body(open(f'{OLD}/diffs/{name}').read()):
        if (h, k) in old_own:
            cand[(s, t)].add(old_own[(h, k)][0])
    nb = body(open(f'{M}/diffs/{name}').read())
    own, files = owners(M, name)
    text = {(h, k): (s, t) for h, k, s, t in nb}
    missing = [(int(r[1]), int(r[2])) for r in rows if r[0] == name]
    for key in missing:
        c = cand.get(text[key], set())
        if len(c) == 1:
            own[key] = (next(iter(c)), 'core', 'realigned diff: owner of every old copy of this line')
    byh = collections.defaultdict(list)
    for h, k, _, _ in nb:
        byh[h].append(k)
    for h, k in missing:
        if (h, k) in own:
            continue
        ks = byh[h]; i = ks.index(k)
        prev = next((own[(h, x)] for x in reversed(ks[:i]) if (h, x) in own), None)
        nxt = next((own[(h, x)] for x in ks[i + 1:] if (h, x) in own), None)
        o = prev or nxt
        own[(h, k)] = (o[0], 'core', 'realigned diff: brace or blank, owner of its neighbour')
        print(f'{name} {h} {k} -> {o[0]}: {text[(h, k)][0]}{text[(h, k)][1][:80]}')
    target = sorted(files)[0]
    for f in files:
        kept = [l for l in open(f).read().split('\n') if not l.startswith(name + '\t')]
        open(f, 'w').write('\n'.join(kept).rstrip('\n') + '\n')
    runs, prev = [], None
    for h, k, _, _ in nb:
        v = own[(h, k)]
        if prev and prev[0] == h and prev[2] == k - 1 and prev[3] == v:
            prev[2] = k
        else:
            prev = [h, k, k, v]; runs.append(prev)
    with open(target, 'a') as f:
        f.write(''.join(f'{name}\t{h}\t{a}-{b}\t{v[0]}\t{v[1]}\t{v[2]}\n' for h, a, b, v in runs))
    print(f'{name}: {len(missing)} filled, {len(runs)} rows')
