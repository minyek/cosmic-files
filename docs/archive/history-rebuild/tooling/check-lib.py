#!/usr/bin/python3 -I
"""check-lib.py: shared baseline/finding helpers (copied from check-commit.py).
check-commit.py <branch> [from-index]: D5(c) for every commit e03001c..<branch>, in a detached check worktree.
Results: commits/check-<sha>.json and commits/check-summary.txt"""
import glob,json,os,re,subprocess,sys
REPO='/mnt/work/projects/cosmic-de/cosmic-files'
WT=REPO+'/.worktrees/history-rebuild-check'
M='/mnt/logs/temp/history-rebuild/map'; OUT='/mnt/logs/temp/history-rebuild/commits'
CFG={'no-default':['--no-default-features'],'default':[],'all':['--all-features']}
def run(cmd,cwd=WT):
    return subprocess.run(cmd,cwd=cwd,capture_output=True,text=True)
FIND=re.compile(r'^(\S+?):\d+:\d+: (warning|error)(?:\[\w+\])?: (.*)$')
def findings(text):
    s=set()
    for ln in text.split('\n'):
        m=FIND.match(ln)
        if m and not m.group(3).startswith(('unused manifest','`cosmic-files`')) and 'generated' not in m.group(3): s.add((m.group(1),m.group(2),m.group(3)[:160]))
    return s
def baseline(kind,cfg):
    s=set()
    for t in ('base','target'):
        f=f'{M}/baseline-{t}-{kind}-{cfg}.txt'
        if os.path.exists(f): s|=findings(open(f).read())
    return s
BASE={('check',c):baseline('check',c) for c in CFG}; BASE[('clippy','default')]=baseline('clippy','default')
