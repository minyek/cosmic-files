#!/usr/bin/python3 -I
"""rewrite-messages.py <base> <branch>: recreate base..branch with identical trees, authors and order,
changing only messages:
  - drop body lines that start 'Landed in ' (fork history, meaningless upstream);
  - for code commits, add 'Depends on: <issues of each direct dependency commit>' before the
    trailers, from commits/deps.tsv (the D6 closure, reduced to direct dependencies).
Moves <branch> to the new tip and prints old -> new for every commit."""
import re, subprocess, sys

REPO = '/mnt/work/projects/cosmic-de/cosmic-files'
DEPS = '/mnt/logs/temp/history-rebuild/commits/deps.tsv'


def git(*args, inp=None, env=None):
    return subprocess.run(['git', '-C', REPO, *args], capture_output=True, text=True, input=inp,
                          env=env, check=True).stdout


base, branch = sys.argv[1], sys.argv[2]
shas = git('rev-list', '--reverse', f'{base}..{branch}').split()
closure = {}
issues = {}
for line in open(DEPS):
    sha8, own, dep = (line.rstrip('\n').split('\t') + [''])[:3]
    issues[sha8] = own.split(',')
    closure[sha8] = set(dep.split(',')) - {''}
unit_of = {n: sha8 for sha8, ns in issues.items() for n in ns}
dep_units = {s: {unit_of[n] for n in closure[s]} for s in closure}


def direct(sha8):
    deps = dep_units[sha8]
    implied = set().union(*(dep_units[d] for d in deps)) if deps else set()
    return sorted(deps - implied, key=lambda u: [s[:8] for s in shas].index(u))


def ref(n):
    return f'minyek/cosmic-files#{n}'


import os
parent = git('rev-parse', base).strip()
for sha in shas:
    msg = git('log', '-1', '--format=%B', sha).rstrip('\n')
    body_lines = [l for l in msg.split('\n') if not l.startswith('Landed in ')]
    msg = '\n'.join(body_lines)
    msg = re.sub(r'\n{3,}', '\n\n', msg)
    if sha[:8] in dep_units:
        d = direct(sha[:8])
        line = 'Depends on: ' + ('; '.join(', '.join(ref(n) for n in issues[u]) for u in d) if d else 'nothing beyond upstream') + '.'
        head, sep, trailers = msg.rpartition('\n\n')
        msg = f'{head}\n\n{line}\n\n{trailers}' if sep else f'{msg}\n\n{line}'
    fmt = git('log', '-1', '--format=%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI', sha).rstrip('\n').split('\x00')
    env = dict(os.environ, GIT_AUTHOR_NAME=fmt[0], GIT_AUTHOR_EMAIL=fmt[1], GIT_AUTHOR_DATE=fmt[2],
               GIT_COMMITTER_NAME=fmt[3], GIT_COMMITTER_EMAIL=fmt[4], GIT_COMMITTER_DATE=fmt[5])
    tree = git('rev-parse', f'{sha}^{{tree}}').strip()
    parent = git('commit-tree', tree, '-p', parent, '-F', '-', inp=msg + '\n', env=env).strip()
    print(sha[:8], '->', parent[:8])
print('tip', parent)
