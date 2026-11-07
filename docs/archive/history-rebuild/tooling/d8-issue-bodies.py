#!/usr/bin/python3 -I
"""d8-issue-bodies.py <series-branch> --dry-run|--apply: D8 / AC 11 edits of minyek/cosmic-files issues.

For every issue named by a Fork-issue trailer on e03001c..<series-branch> (never #58):
  - 'Landed in <old>…' becomes 'Landed in <new sha12> (before the history rebuild: <old>…, tag
    backup/master-pre-history-2026-11)'; a body without the line gets one ('Landed in: to be
    filled in…' becomes 'Landed in <new sha12>.');
  - for the issues in CORRECTED, the Problem/Cause/Fix sections are replaced by the corrected
    local copies in map/issues/ (review findings), and #61's title follows its local copy.
--dry-run writes <out>/<n>.before.md, <n>.after.md and a unified diff summary; --apply edits GitHub
with `gh issue edit` and records #58's updatedAt before and after."""
import difflib, json, os, re, subprocess, sys

REPO = '/mnt/work/projects/cosmic-de/cosmic-files'
GH = 'minyek/cosmic-files'
M = '/mnt/logs/temp/history-rebuild/map'
OUT = '/mnt/logs/temp/history-rebuild/d8'
TAG = 'backup/master-pre-history-2026-11'
OLD_FOR_MISSING = {'56': 'f4ff95d', '59': '66dac3f', '60': '66dac3f', '61': '66dac3f'}
CORRECTED = {'27', '34', '40', '59', '61'}


def run(*a):
    return subprocess.run(a, capture_output=True, text=True, check=True).stdout


def sections(text):
    return {m.group(1).strip(): m.group(2) for m in re.finditer(r'^## (.+?)\n(.*?)(?=^## |\Z)', text, re.S | re.M)}


def new_body(n, body, sha):
    old_landed = next((l[len('Landed in '):].rstrip('.') for l in body.split('\n')
                       if l.startswith('Landed in ') and not l.startswith('Landed in: to be filled in')), None)
    if n in CORRECTED:
        local = sections(open(f'{M}/issues/{n}.md').read())
        for name in ('Problem', 'Cause', 'Fix'):
            if name in local:
                body = re.sub(r'(^## ' + re.escape(name) + r'\n)(.*?)(?=^## |\Z)',
                              lambda m: m.group(1) + local[name], body, count=1, flags=re.S | re.M)
    lines = body.split('\n')
    hit = False
    for i, l in enumerate(lines):
        if l.startswith('Landed in: to be filled in'):
            old = OLD_FOR_MISSING.get(n)
            lines[i] = f'Landed in {sha}' + (f' (before the history rebuild: {old}, tag `{TAG}`)' if old else '') + '.'; hit = True
        elif l.startswith('Landed in '):
            old = l[len('Landed in '):].rstrip('.')
            lines[i] = f'Landed in {sha} (before the history rebuild: {old}, tag `{TAG}`).'; hit = True
    body = '\n'.join(lines)
    if not hit:
        old = old_landed or OLD_FOR_MISSING.get(n)
        note = f' (before the history rebuild: {old}, tag `{TAG}`)' if old else ''
        body = body.rstrip('\n') + f'\n\nLanded in {sha}{note}.\n'
    return body


def main():
    branch, mode = sys.argv[1], sys.argv[2]
    os.makedirs(OUT, exist_ok=True)
    commit_of = {}
    for sha in run('git', '-C', REPO, 'rev-list', f'e03001c..{branch}').split():
        msg = run('git', '-C', REPO, 'log', '-1', '--format=%B', sha)
        for n in re.findall(r'^Fork-issue: minyek/cosmic-files#(\d+)$', msg, re.M):
            commit_of[n] = sha[:12]
    assert '58' not in commit_of
    before58 = json.loads(run('gh', 'issue', 'view', '58', '-R', GH, '--json', 'updatedAt'))['updatedAt']
    summary = []
    for n in sorted(commit_of, key=int):
        data = json.loads(run('gh', 'issue', 'view', n, '-R', GH, '--json', 'body,title'))
        after = new_body(n, data['body'], commit_of[n])
        title = data['title']
        if n == '61':
            title = re.sub(r'^# #\d+ | \[\w+\]$', '', open(f'{M}/issues/61.md').read().split('\n')[0])
        open(f'{OUT}/{n}.before.md', 'w').write(data['body'])
        open(f'{OUT}/{n}.after.md', 'w').write(after)
        diff = list(difflib.unified_diff(data['body'].split('\n'), after.split('\n'), f'#{n} before', f'#{n} after', lineterm='', n=0))
        summary.append('\n'.join(diff) + (f'\ntitle: {data["title"]!r} -> {title!r}' if title != data['title'] else ''))
        if mode == '--apply':
            args = ['gh', 'issue', 'edit', n, '-R', GH, '--body-file', f'{OUT}/{n}.after.md']
            if title != data['title']:
                args += ['--title', title]
            run(*args)
    open(f'{OUT}/summary.diff', 'w').write('\n\n'.join(summary) + '\n')
    after58 = json.loads(run('gh', 'issue', 'view', '58', '-R', GH, '--json', 'updatedAt'))['updatedAt']
    print(f'{len(commit_of)} issues; #58 updatedAt {before58} -> {after58}; summary {OUT}/summary.diff')
    json.dump(commit_of, open(f'{OUT}/commit-map.json', 'w'), indent=1)


if __name__ == '__main__':
    main()
