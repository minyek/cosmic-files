#!/usr/bin/python3 -I
"""check-series.py <repo> <base> <tip> <issues-file>: AC 1, 3, 4 and 12 over base..tip.

<issues-file> lists, one per line, every issue number whose code is in the net diff.
The last four commits are the D3 fork-only commits; every earlier one is a code commit.
Prints one line per violation and exits 1 if there are any."""
import re, subprocess, sys

UPSTREAM_PATH = re.compile(r'^(src/|tests/|i18n/|res/|Cargo\.toml$|Cargo\.lock$|TESTING\.md$)')
BARE_ISSUE = re.compile(r'(^|[^A-Za-z0-9_/-])#[0-9]+')
# In code, a colour such as #303030 is not an issue reference: issue numbers have at most three digits.
CODE_ISSUE = re.compile(r'(^|[^A-Za-z0-9_/-])#[0-9]{1,3}(?![0-9A-Za-z])')
SUBJECT_PR = re.compile(r'\(#[0-9]+\)')
TRAILER = re.compile(r'^Fork-issue: minyek/cosmic-files#([0-9]+)$', re.M)
FORK_ONLY_COMMITS = int(__import__("os").environ.get("FORK_ONLY_COMMITS", "4"))
TOOLING_EXCEPTION = 'src/lib.rs'


def git(repo, *args):
    return subprocess.run(['git', '-C', repo, *args], capture_output=True, text=True, check=True).stdout


def added_lines(repo, sha):
    out = git(repo, 'show', '--format=', '--unified=0', '--no-color', sha)
    path = None
    for line in out.split('\n'):
        if line.startswith('+++ '):
            path = line[6:] if line.startswith('+++ b/') else None
        elif line.startswith('+') and path:
            yield path, line[1:]


def check(repo, base, tip, issues):
    bad = []
    if subprocess.run(['git', '-C', repo, 'merge-base', '--is-ancestor', base, tip]).returncode:
        bad.append(f'AC1: {base} is not an ancestor of {tip}')
    if git(repo, 'rev-list', '--merges', f'{base}..{tip}').strip():
        bad.append('AC1: merge commits in the series')
    shas = git(repo, 'rev-list', '--reverse', f'{base}..{tip}').split()
    code, fork = shas[:len(shas) - FORK_ONLY_COMMITS], shas[len(shas) - FORK_ONLY_COMMITS:]
    seen = {}
    for sha in shas:
        short = sha[:8]
        msg = git(repo, 'log', '-1', '--format=%B', sha)
        subject = msg.split('\n', 1)[0]
        if SUBJECT_PR.search(subject):
            bad.append(f'AC12 {short}: (#N) in subject')
        for n, line in enumerate(msg.split('\n'), 1):
            if BARE_ISSUE.search(line):
                bad.append(f'AC12 {short}: bare #N in message line {n}: {line[:80]}')
            if line.startswith('Claude-Session') or line.startswith('Signed-off-by'):
                bad.append(f'AC12 {short}: forbidden trailer: {line[:60]}')
        trailers = TRAILER.findall(msg)
        paths = git(repo, 'show', '--format=', '--name-only', sha).split()
        if sha in code:
            if not trailers:
                bad.append(f'AC3 {short}: code commit without a Fork-issue trailer')
            for n in trailers:
                if n in seen:
                    bad.append(f'AC3 {short}: #{n} already in {seen[n]}')
                seen[n] = short
            for p in paths:
                if not UPSTREAM_PATH.match(p):
                    bad.append(f'AC4 {short}: fork-only path in a code commit: {p}')
            for p, line in added_lines(repo, sha):
                if CODE_ISSUE.search(line) or 'minyek' in line or re.search(r'\bdocs/', line):
                    bad.append(f'AC12 {short}: fork-specific text added in {p}: {line.strip()[:80]}')
        else:
            if trailers:
                bad.append(f'AC3 {short}: fork-only commit has Fork-issue trailers')
            for p in paths:
                if UPSTREAM_PATH.match(p) and not (p == TOOLING_EXCEPTION and sha == fork[0]):
                    bad.append(f'AC4 {short}: upstream-shaped path in a fork-only commit: {p}')
    for n in sorted(issues - set(seen), key=int):
        bad.append(f'AC3: issue #{n} has code but no commit')
    for n in sorted(set(seen) - issues, key=int):
        bad.append(f'AC3: commit {seen[n]} names #{n}, which has no code in the net diff')
    return bad


if __name__ == '__main__':
    repo, base, tip, issues_file = sys.argv[1:5]
    issues = {l.strip() for l in open(issues_file) if l.strip()}
    problems = check(repo, base, tip, issues)
    print('\n'.join(problems) if problems else f'check-series: OK ({base}..{tip})')
    sys.exit(1 if problems else 0)
