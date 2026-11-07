#!/usr/bin/python3 -I
"""remap-map.py <old-target> <new-target> <path>...: regenerate map/diffs for <path> against the new
target and carry every row keyed by (diff, hunk, line) — assign/*.tsv, overrides.tsv, variants.tsv,
keep-variants.tsv — over to the new diff by matching diff lines on (sign, text). New lines with no
match are written to map/unassigned-new.tsv with their neighbours' owners, for a hand assignment.
Run after copying map/ to map-v1/."""
import difflib, glob, os, subprocess, sys

REPO = '/mnt/work/projects/cosmic-de/cosmic-files'
M = '/mnt/logs/temp/history-rebuild/map'
old_t, new_t, paths = sys.argv[1], sys.argv[2], sys.argv[3:]


def body(text):
    """[(hunk, k, sign, text)] for a -U0 diff."""
    out, h, k = [], 0, 0
    for ln in text.split('\n'):
        if ln.startswith('@@'):
            h += 1; k = 0
        elif h and ln and ln[0] in '+-\\':
            k += 1; out.append((h, k, ln[0], ln[1:]))
    return out


def rows_of(f):
    for l in open(f):
        p = l.rstrip('\n').split('\t')
        yield l, p


unassigned = open(M + '/unassigned-new.tsv', 'w')
for path in paths:
    name = path.replace('/', '_') + '.diff'
    old_text = open(f'{M}/diffs/{name}').read()
    alg = os.environ.get('DIFF_ALG', 'myers')
    new_text = subprocess.run(['git', '-C', REPO, 'diff', '-U0', f'--diff-algorithm={alg}', 'e03001c', new_t, '--', path],
                              capture_output=True, text=True, check=True).stdout
    ob, nb = body(old_text), body(new_text)
    sm = difflib.SequenceMatcher(None, [(s, t) for _, _, s, t in ob], [(s, t) for _, _, s, t in nb], autojunk=False)
    key = {}  # (old h, k) -> (new h, k)
    for a, b, size in sm.get_matching_blocks():
        for i in range(size):
            key[(ob[a + i][0], ob[a + i][1])] = (nb[b + i][0], nb[b + i][1])
    matched_new = set(key.values())
    # per-line owners from the assign files
    owner = {}
    for f in glob.glob(M + '/assign/*.tsv'):
        if f.endswith('.deps.tsv'):
            continue
        for _, p in rows_of(f):
            if len(p) >= 5 and p[0] == name:
                lo, hi = map(int, p[2].split('-'))
                for k in range(lo, hi + 1):
                    owner[(int(p[1]), k)] = (p[3], p[4], p[5] if len(p) > 5 else '')
    new_owner = {key[o]: v for o, v in owner.items() if o in key}
    # rewrite the assign rows for this diff: drop the old rows, append the remapped runs
    target_file = None
    for f in glob.glob(M + '/assign/*.tsv'):
        if f.endswith('.deps.tsv'):
            continue
        lines = open(f).read().split('\n')
        kept = [l for l in lines if not l.startswith(name + '\t')]
        if len(kept) != len(lines):
            target_file = target_file or f
            open(f, 'w').write('\n'.join(kept))
    runs, prev = [], None
    for h, k, s, t in nb:
        v = new_owner.get((h, k))
        if v is None:
            unassigned.write(f'{name}\t{h}\t{k}\t{s}{t[:120]}\n')
            prev = None
            continue
        if prev and prev[0] == h and prev[2] == k - 1 and prev[3] == v:
            prev[2] = k
        else:
            prev = [h, k, k, v]; runs.append(prev)
    if target_file is None:
        sys.exit(f'no assign file holds rows for {name}')
    with open(target_file, 'a') as f:
        f.write('\n' + '\n'.join(f'{name}\t{h}\t{a}-{b}\t{v[0]}\t{v[1]}\t{v[2]}' for h, a, b, v in runs) + '\n')
    # keyed side files
    for side in ('overrides.tsv', 'variants.tsv', 'keep-variants.tsv'):
        sp = f'{M}/{side}'
        if not os.path.exists(sp):
            continue
        out = []
        for l, p in rows_of(sp):
            if len(p) >= 3 and p[0] == name and not p[0].startswith('#'):
                if side == 'overrides.tsv':
                    lo, hi = map(int, p[2].split('-'))
                    news = [key[(int(p[1]), k)] for k in range(lo, hi + 1) if (int(p[1]), k) in key]
                    if len(news) != hi - lo + 1 or len({n[0] for n in news}) != 1:
                        print(f'{side}: cannot remap {l.strip()}'); out.append(l); continue
                    p[1], p[2] = str(news[0][0]), f'{news[0][1]}-{news[-1][1]}'
                else:
                    n = key.get((int(p[1]), int(p[2])))
                    if n is None:
                        print(f'{side}: cannot remap {l.strip()}'); out.append(l); continue
                    p[1], p[2] = str(n[0]), str(n[1])
                out.append('\t'.join(p) + '\n')
            else:
                out.append(l)
        open(sp, 'w').write(''.join(out))
    open(f'{M}/diffs/{name}', 'w').write(new_text)
    print(f'{name}: old lines {len(ob)}, new {len(nb)}, matched {len(matched_new)}, '
          f'new unmatched {len(nb) - len(matched_new)}, old owners carried {len(new_owner)}')
