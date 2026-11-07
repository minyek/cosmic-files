#!/usr/bin/python3 -I
"""check-selfcontained.py <base> <tip> [first-index]: D6 for every code commit (all but the last four).

A commit's declared dependencies are the transitive closure of
  - textual: series commits that blame names for the old-side lines (removed lines and three lines
    of context) of the commit's diff, and
  - semantic: map/semantic-deps.tsv, '<dependent issue>\t<needed issue>\t<reason>', by Fork-issue.
Each code commit is replayed onto <base> after only its dependencies, in series order, with
`git merge-tree --write-tree`, and the resulting tree must pass default-config
`cargo check --locked --all-targets` in the check worktree.
Writes commits/selfcontained.tsv and commits/deps.tsv (sha, issues, dependency issues)."""
import re, subprocess, sys

REPO = '/mnt/work/projects/cosmic-de/cosmic-files'
WT = REPO + '/.worktrees/history-rebuild-check'
M = '/mnt/logs/temp/history-rebuild/map'
OUT = '/mnt/logs/temp/history-rebuild/commits'
ALG = '--diff-algorithm=histogram'
FORK_ONLY_COMMITS = int(__import__("os").environ.get("FORK_ONLY_COMMITS", "4"))


def git(*args, cwd=REPO, check=True, inp=None):
    r = subprocess.run(['git', *args], cwd=cwd, capture_output=True, text=True, input=inp,
                       errors='surrogateescape')
    if check and r.returncode:
        raise RuntimeError(f'git {args[:3]}: {r.stderr[-300:]}')
    return r


def issues_of(sha):
    return re.findall(r'^Fork-issue: minyek/cosmic-files#(\d+)$', git('log', '-1', '--format=%B', sha).stdout, re.M)


def textual_deps(sha, series):
    deps = set()
    diff = git('show', ALG, '--format=', '--unified=3', '--no-color', sha).stdout
    path = None
    for line in diff.split('\n'):
        if line.startswith('--- '):
            path = line[6:] if line.startswith('--- a/') else None
        m = re.match(r'@@ -(\d+)(?:,(\d+))? ', line)
        if m and path:
            start, count = int(m.group(1)), int(m.group(2) or 1)
            if count == 0:
                continue
            out = git('blame', ALG, '--porcelain', '-L', f'{start},{start + count - 1}', f'{sha}^', '--', path).stdout
            for b in out.split('\n'):
                h = b.split(' ')[0]
                if len(h) == 40 and h in series:
                    deps.add(h)
    return deps


def main():
    base, tip = sys.argv[1], sys.argv[2]
    first = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    shas = git('rev-list', '--reverse', f'{base}..{tip}').stdout.split()
    code = shas[:len(shas) - FORK_ONLY_COMMITS]
    index = {s: i for i, s in enumerate(code)}
    by_issue = {n: s for s in code for n in issues_of(s)}
    semantic = {}
    for l in open(M + '/semantic-deps.tsv'):
        p = l.rstrip('\n').split('\t')
        if len(p) >= 2 and not p[0].startswith('#'):
            semantic.setdefault(p[0], set()).add(p[1])
    direct = {}
    for s in code:
        d = textual_deps(s, set(code))
        for n in issues_of(s):
            for x in semantic.get(n, ()):
                if index[by_issue[x]] > index[s]:
                    print(f'warning: #{n} declares #{x}, which comes later; ignored', flush=True)
                else:
                    d.add(by_issue[x])
        direct[s] = d - {s}
    closure = {}
    for s in code:  # series order, so every dependency's closure is already known
        c = set()
        for d in direct[s]:
            c |= {d} | closure[d]
        closure[s] = c
    with open(OUT + '/deps.tsv', 'w') as f:
        for s in code:
            dn = sorted({n for d in closure[s] for n in issues_of(d)}, key=int)
            f.write(f"{s[:8]}\t{','.join(issues_of(s))}\t{','.join(dn)}\n")
    summ = open(OUT + '/selfcontained.tsv', 'a')
    for i, s in enumerate(code, 1):
        if i < first:
            continue
        cur = git('rev-parse', base).stdout.strip()
        conflict = None
        for d in sorted(closure[s] | {s}, key=lambda x: index[x]):
            r = git('merge-tree', '--write-tree', '--merge-base', f'{d}^', cur, d, check=False)
            if r.returncode:
                conflict = f'conflict applying {d[:8]}: ' + ' '.join(
                    x for x in r.stdout.split('\n')[1:] if x.startswith('CONFLICT'))[:300]
                break
            tree = r.stdout.split('\n')[0]
            cur = git('commit-tree', tree, '-p', cur, '-m', f'replay {d[:8]}').stdout.strip()
        if conflict:
            summ.write(f'{i:02d}\t{s[:8]}\tCONFLICT\t{conflict}\n'); summ.flush(); continue
        git('checkout', '-q', '--detach', cur, cwd=WT)
        git('clean', '-fdq', cwd=WT)
        r = subprocess.run(['cargo', 'check', '--locked', '--all-targets', '--message-format=short'],
                           cwd=WT, capture_output=True, text=True)
        errs = [l for l in (r.stdout + r.stderr).split('\n') if ': error' in l][:5]
        summ.write(f"{i:02d}\t{s[:8]}\t{'OK' if r.returncode == 0 else 'BUILD-FAIL'}\t"
                   f"deps={len(closure[s])}\t{' | '.join(errs)[:400]}\n")
        summ.flush()


if __name__ == '__main__':
    main()
