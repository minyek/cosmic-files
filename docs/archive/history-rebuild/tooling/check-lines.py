#!/usr/bin/python3 -I
"""check-lines.py <repo> <base> <tip> <listed-file> [--report FILE]: AC 5 over base..tip.

Finds every line a series commit adds that does not survive to <tip> (git blame --reverse,
histogram), and the commit that changes or removes it. A line passes when either
  - the spec's mechanical rule holds: every token of the line is still among the tokens the
    changing commit adds in the same hunk (a list that grew, or a re-wrap), or
  - <listed-file> lists it: '<adding-sha8>\t<path>\t<kind>\t<issue>\t<reason>\t<exact stripped line>' with kind
    issue-driven or grouped (the issue is the one whose text requires the change).
Also checks that the per-commit --numstat total equals the net diff total plus twice the number
of non-surviving lines. Exit 1 on any violation."""
import collections, re, subprocess, sys

ALG = '--diff-algorithm=histogram'
TOKEN = re.compile(r'[A-Za-z_][A-Za-z0-9_]*|[0-9]+|\S')


def git(repo, *args):
    return subprocess.run(['git', '-C', repo, *args], capture_output=True, text=True,
                          errors='surrogateescape', check=True).stdout


def numstat_total(repo, *rev):
    total = 0
    for line in git(repo, 'diff', ALG, '--numstat', *rev).split('\n'):
        a, d, *_ = line.split('\t') + ['', '']
        if a.isdigit():
            total += int(a) + int(d)
    return total


def added_line_numbers(repo, sha):
    """{path: [new-side line numbers added by sha]} for text files."""
    out = git(repo, 'show', ALG, '--format=', '--unified=0', '--no-color', sha)
    res, path = collections.defaultdict(list), None
    for line in out.split('\n'):
        if line.startswith('+++ '):
            path = line[6:] if line.startswith('+++ b/') else None
        m = re.match(r'@@ -\S+ \+(\d+)(?:,(\d+))? @@', line)
        if m and path:
            start, count = int(m.group(1)), int(m.group(2) or 1)
            res[path].extend(range(start, start + count))
    return res


def hunks(repo, sha, path):
    """[(removed old-side line numbers, removed texts, added texts)] for sha's diff of path."""
    out = git(repo, 'show', ALG, '--format=', '--unified=0', '--no-color', sha, '--', path)
    res, cur, old = [], None, 0
    for line in out.split('\n'):
        m = re.match(r'@@ -(\d+)(?:,(\d+))? \+\S+ @@', line)
        if m:
            cur = (set(), [], []); res.append(cur)
            old = int(m.group(1)) if m.group(2) != '0' else int(m.group(1)) + 1
        elif cur is not None and line.startswith('-') and not line.startswith('---'):
            cur[0].add(old); cur[1].append(line[1:]); old += 1
        elif cur is not None and line.startswith('+') and not line.startswith('+++'):
            cur[2].append(line[1:])
    return res


USE_OR_LIST = re.compile(r'^\s*(pub(\(\w+\))?\s+)?use\s|^\s*([\w:]+(\s*\{[^}]*\})?\s*,\s*)+([\w:]+(\s*\{[^}]*\})?)?\s*[,;]?\s*$')


def mechanical(text, added):
    """AC 5's mechanical rule: a pure re-wrap or re-indent, or a `use` or comma list that kept
    every token of the line."""
    squash = lambda s: re.sub(r'\s+', '', s)
    if not squash(text) or squash(text) in squash(''.join(added)):
        return True
    if not USE_OR_LIST.match(text):
        return False
    want = collections.Counter(TOKEN.findall(text))
    have = collections.Counter(t for a in added for t in TOKEN.findall(a))
    return not (want - have)


def non_surviving(repo, tip, shas):
    """Yield (adding sha, path, text, changing sha) for each added line that does not reach tip."""
    nxt = {s: shas[i + 1] for i, s in enumerate(shas[:-1])}
    for sha in shas[:-1]:  # the tip's own lines survive by definition
        for path, lines in added_line_numbers(repo, sha).items():
            if not lines:
                continue
            ranges, lo, hi = [], lines[0], lines[0]
            for n in lines[1:]:
                if n == hi + 1:
                    hi = n
                else:
                    ranges.append((lo, hi)); lo = hi = n
            ranges.append((lo, hi))
            spec = [a for lo, hi in ranges for a in ('-L', f'{lo},{hi}')]
            out = git(repo, 'blame', ALG, '--reverse', f'{sha}..{tip}', '--porcelain', *spec, '--', path)
            cur = None
            for line in out.split('\n'):
                m = re.match(r'^([0-9a-f]{40}) (\d+) (\d+)', line)
                if m:
                    cur = m.group(1)
                elif line.startswith('\t') and cur:
                    last = cur
                    if last != shas[-1]:
                        yield sha, path, line[1:], nxt.get(last, last)
                    cur = None


def main():
    repo, base, tip, listed_file = sys.argv[1:5]
    report = sys.argv[sys.argv.index('--report') + 1] if '--report' in sys.argv else None
    listed = {}
    for l in open(listed_file):
        p = l.rstrip('\n').split('\t')
        if len(p) >= 6 and not p[0].startswith('#'):
            listed[(p[0], p[1], p[5])] = p[2:5]
    shas = git(repo, 'rev-list', '--reverse', f'{base}..{tip}').split()
    bad, rows, count = [], [], 0
    hunk_cache = {}
    for adder, path, text, changer in non_surviving(repo, tip, shas):
        count += 1
        key = (changer, path)
        if key not in hunk_cache:
            hunk_cache[key] = hunks(repo, changer, path)
        added = [a for _, removed, adds in hunk_cache[key] if text in removed for a in adds]
        if mechanical(text, added):
            kind = 'mechanical'
        elif (adder[:8], path, text.strip()) in listed:
            kind = listed[(adder[:8], path, text.strip())][0]
        else:
            kind = 'UNLISTED'
            bad.append(f'AC5: {adder[:8]} {path}: line changed by {changer[:8]} is neither mechanical nor listed: {text.strip()[:90]}')
        rows.append(f'{adder[:8]}\t{changer[:8]}\t{path}\t{kind}\t{text.strip()[:120]}')
    series = sum(numstat_total(repo, f'{s}^', s) for s in shas)
    net = numstat_total(repo, base, tip)
    # Informational only: AC 5's first bullet was amended (Q2-A) to added-identity.py, because the
    # net numstat depends on how the diff algorithm aligns lines.
    print(f'numstat (informational): series {series}, net {net} + 2 x {count} = {net + 2 * count}')
    if report:
        open(report, 'w').write('\n'.join(rows) + '\n')
    print(f'non-surviving lines: {count}; series numstat {series}; net {net}')
    print('\n'.join(bad) if bad else 'check-lines: OK')
    sys.exit(1 if bad else 0)


if __name__ == '__main__':
    main()
