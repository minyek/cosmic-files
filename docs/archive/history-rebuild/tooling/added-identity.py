#!/usr/bin/python3 -I
"""added-identity.py <repo> <base> <tip> <non-surviving-count>: alignment-independent form of AC 5's
numstat cross-check. Lines added across the series' commits must equal the tip's lines that blame
attributes to series commits plus the lines that do not survive."""
import subprocess, sys

repo, base, tip, nonsurv = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
ALG = '--diff-algorithm=histogram'


def git(*args):
    return subprocess.run(['git', '-C', repo, *args], capture_output=True, text=True,
                          errors='surrogateescape', check=True).stdout


shas = git('rev-list', f'{base}..{tip}').split()
added = 0
for sha in shas:
    for line in git('diff', ALG, '--numstat', f'{sha}^', sha).split('\n'):
        p = line.split('\t')
        if len(p) == 3 and p[0].isdigit():
            added += int(p[0])
series = set(shas)
blamed = 0
for path in git('diff', '--name-only', '--diff-filter=AM', base, tip).split('\n'):
    if not path:
        continue
    if git('diff', '--numstat', base, tip, '--', path).startswith('-'):
        continue  # binary
    for line in git('blame', ALG, '--porcelain', f'{base}..{tip}', '--', path).split('\n'):
        p = line.split(' ')
        if len(p) >= 3 and len(p[0]) == 40 and p[0] in series and p[2].isdigit():
            blamed += 1
print(f'added {added}; blamed at tip {blamed}; non-surviving {nonsurv}; '
      f"{'OK' if added == blamed + nonsurv else 'MISMATCH'}")
sys.exit(0 if added == blamed + nonsurv else 1)
