#!/usr/bin/python3 -I
"""check-issues.py <before|after>: AC 11. 'before' checks the dry run's after-bodies; 'after' checks the
live GitHub bodies. Every issue in d8/commit-map.json names its commit ('Landed in <sha12>'), and the
sha12 is the commit whose Fork-issue trailer names that issue. #58 must not be in the map."""
import json, subprocess, sys

OUT = '/mnt/logs/temp/history-rebuild/d8'
mode = sys.argv[1]
commit_of = json.load(open(f'{OUT}/commit-map.json'))
bad = []
if '58' in commit_of:
    bad.append('#58 is in the commit map')
for n, sha in commit_of.items():
    if mode == 'after':
        body = json.loads(subprocess.run(['gh', 'issue', 'view', n, '-R', 'minyek/cosmic-files', '--json', 'body'],
                                         capture_output=True, text=True, check=True).stdout)['body']
    else:
        body = open(f'{OUT}/{n}.after.md').read()
    if f'Landed in {sha}' not in body:
        bad.append(f'#{n}: no "Landed in {sha}"')
    if 'to be filled in' in body:
        bad.append(f'#{n}: placeholder left')
print('\n'.join(bad) if bad else f'check-issues {mode}: OK ({len(commit_of)} issues)')
sys.exit(1 if bad else 0)
