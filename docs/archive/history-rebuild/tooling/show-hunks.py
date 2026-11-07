#!/usr/bin/python3 -I
"""show-hunks.py <diff-name> <hunk>...: print each diff line with its (hunk, k) key and owners."""
import glob, sys
M = '/mnt/logs/temp/history-rebuild/map'
name, want = sys.argv[1], {int(h) for h in sys.argv[2:]}
own = {}
for f in glob.glob(M + '/assign/*.tsv'):
    if f.endswith('deps.tsv'):
        continue
    for l in open(f):
        p = l.rstrip('\n').split('\t')
        if len(p) >= 5 and p[0] == name:
            a, b = map(int, p[2].split('-'))
            for k in range(a, b + 1):
                own[(int(p[1]), k)] = p[3]
h = 0
for ln in open(f'{M}/diffs/{name}').read().split('\n'):
    if ln.startswith('@@'):
        h += 1; k = 0
        if h in want:
            print(ln[:90])
        continue
    if h and ln and ln[0] in '+-\\':
        k += 1
        if h in want:
            print(f'{h:>3} {k:>3} {own.get((h, k), "?"):<8} {ln[:110]}')
