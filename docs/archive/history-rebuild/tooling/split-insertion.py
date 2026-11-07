#!/usr/bin/python3 -I
"""split-insertion.py <diff-name> <hunk> <j>: split a pure insertion hunk around the base line after it.

A -U0 diff may insert I[0..n] after base line L where I[j-1] equals base line L+1 (a closing brace
the diff matched to the wrong block). The same final text is base[..L] + I[:j-1] + base[L+1] +
I[j:] + I[j-1] + base[L+2..]. This rewrites the hunk as two insertions, at L and at L+1, so the
base line closes the block it closed upstream, and moves the moved line's row to the end of the
second hunk with the owner of the line before it. Later hunks are renumbered in map/assign/*.tsv,
overrides.tsv, variants.tsv and keep-variants.tsv. <j> is 1-based, as in the map's line numbers."""
import glob, os, re, subprocess, sys

M = '/mnt/logs/temp/history-rebuild/map'
REPO = '/mnt/work/projects/cosmic-de/cosmic-files'
name, H, J = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
path = open(f'{M}/diffs/{name}').read()
lines = path.split('\n')
# locate hunk H
idx = [i for i, l in enumerate(lines) if l.startswith('@@')]
start = idx[H - 1]
end = idx[H] if H < len(idx) else len(lines)
m = re.match(r'@@ -(\d+),0 \+(\d+)(?:,(\d+))? @@(.*)$', lines[start])
if not m:
    sys.exit('hunk is not a pure insertion')
L, newstart, tail = int(m.group(1)), int(m.group(2)), m.group(4)
body = [l for l in lines[start + 1:end] if l.startswith('+')]
n = len(body)
src_path = next(l[6:] for l in lines if l.startswith('--- a/'))
base_next = subprocess.run(['git', '-C', REPO, 'show', f'e03001c:{src_path}'], capture_output=True,
                           text=True, check=True).stdout.split('\n')[L]  # base line L+1 (0-based L)
if body[J - 1][1:] != base_next:
    sys.exit(f'line {J} {body[J - 1]!r} differs from base line {L + 1} {base_next!r}')
first, moved, rest = body[:J - 1], body[J - 1], body[J:]
h1 = f'@@ -{L},0 +{newstart},{len(first)} @@{tail}'
h2 = f'@@ -{L + 1},0 +{newstart + len(first) + 1},{len(rest) + 1} @@{tail}'
new = lines[:start] + [h1] + first + [h2] + rest + [moved] + lines[end:]
open(f'{M}/diffs/{name}', 'w').write('\n'.join(new))


def rekey(h, k):
    """Old (hunk, line) -> new (hunk, line)."""
    if h < H:
        return h, k
    if h > H:
        return h + 1, k
    if k < J:
        return H, k
    if k == J:
        return H + 1, n - J + 1
    return H + 1, k - J


owner_before_moved = None
for f in glob.glob(M + '/assign/*.tsv'):
    if f.endswith('.deps.tsv'):
        continue
    out = []
    changed = False
    for l in open(f).read().split('\n'):
        p = l.split('\t')
        if len(p) >= 5 and p[0] == name and p[1].isdigit():
            h = int(p[1]); a, b = map(int, p[2].split('-'))
            changed = True
            for k in range(a, b + 1):  # one row per line, merged later by readers
                nh, nk = rekey(h, k)
                q = list(p); q[1] = str(nh); q[2] = f'{nk}-{nk}'
                if h == H and k == n:
                    owner_before_moved = q[3:]
                out.append('\t'.join(q))
        else:
            out.append(l)
    if changed:
        open(f, 'w').write('\n'.join(out))
# the moved line takes the owner of the line before it (the last inserted line)
if owner_before_moved is None:
    sys.exit('no row found for the last inserted line')
for f in glob.glob(M + '/assign/*.tsv'):
    if f.endswith('.deps.tsv'):
        continue
    s = open(f).read().split('\n')
    key = f'{name}\t{H + 1}\t{n - J + 1}-{n - J + 1}\t'
    for i, l in enumerate(s):
        if l.startswith(key):
            p = l.split('\t')
            p[3:] = owner_before_moved[:1] + ['list', 'closing brace moved after the inserted arms (split-insertion)']
            s[i] = '\t'.join(p)
            open(f, 'w').write('\n'.join(s))
for side in ('overrides.tsv', 'variants.tsv', 'keep-variants.tsv'):
    sp = f'{M}/{side}'
    if not os.path.exists(sp):
        continue
    out = []
    for l in open(sp).read().split('\n'):
        p = l.split('\t')
        if len(p) >= 3 and p[0] == name and p[1].isdigit():
            if side == 'overrides.tsv':
                a, b = map(int, p[2].split('-'))
                ka, kb = rekey(int(p[1]), a), rekey(int(p[1]), b)
                if ka[0] != kb[0]:
                    sys.exit(f'override spans the split: {l}')
                p[1], p[2] = str(ka[0]), f'{ka[1]}-{kb[1]}'
            else:
                nh, nk = rekey(int(p[1]), int(p[2]))
                p[1], p[2] = str(nh), str(nk)
            out.append('\t'.join(p))
        else:
            out.append(l)
    open(sp, 'w').write('\n'.join(out))
print(f'split {name} hunk {H} at line {J}: {len(first)} + {len(rest)} + moved line; '
      f'moved line now owned by {owner_before_moved[0]}')
