#!/usr/bin/python3 -I
"""check-commit.py <branch> [from-index]: D5(c) for every commit e03001c..<branch>, in a detached check worktree.
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
def issues_of(sha):
    msg=run(['git','log','-1','--format=%B',sha],REPO).stdout
    return re.findall(r'Fork-issue: minyek/cosmic-files#(\d+)',msg)
def named_tests(ns):
    names=set()
    for n in ns:
        t=open(f'{M}/issues/{n}.md').read()
        m=re.search(r'^## Tests\n(.*?)(?=^## |\Z)',t,re.S|re.M)
        if m and not m.group(1).strip().startswith('None'):
            names|=set(re.findall(r'`(?:[a-z_][a-z0-9_]*::)*([a-z][a-z0-9_]{6,})`',m.group(1)))
    ex={l.split('\t')[0] for l in open(M+'/named-test-exempt.tsv') if l.strip()}
    return names-ex
def fmt_hunks():
    r=run(['cargo','fmt','--check'])
    hunks=re.split(r'(?m)^(?=Diff in )',r.stdout)
    return [h[:200] for h in hunks if h.startswith('Diff in') and 'ItemMetadata::Trash' not in h]
shas=run(['git','rev-list','--reverse','e03001c..'+sys.argv[1]],REPO).stdout.split()
start=int(sys.argv[2]) if len(sys.argv)>2 else 1
summ=open(OUT+'/check-summary.txt','a')
for i,sha in enumerate(shas,1):
    if i<start: continue
    res={'sha':sha,'index':i,'subject':run(['git','log','-1','--format=%s',sha],REPO).stdout.strip()}
    run(['git','checkout','-q','--detach',sha])
    run(['git','clean','-fdq'])
    ok=True
    t0=__import__('time').time()-2
    for cfg,flags in CFG.items():
        r=run(['cargo','check','--locked','--all-targets','--message-format=short']+flags)
        new=findings(r.stdout+r.stderr)-BASE[('check',cfg)]
        res['check-'+cfg]={'rc':r.returncode,'new':sorted(new)[:10]}
        ok&= r.returncode==0 and not new
    r=run(['cargo','clippy','--locked','--all-targets','--keep-going','--message-format=short'])
    new=findings(r.stdout+r.stderr)-BASE[('clippy','default')]
    res['clippy']={'new':sorted(new)[:10]}; ok&=not new
    dep=set()
    for d in glob.glob(WT+'/target/debug/deps/cosmic_files-*.d'):
        if os.path.getmtime(d)<t0: continue
        for tok in open(d).read().split():
            tok=tok.rstrip(':').removeprefix(WT+'/')
            if tok.startswith('src/') and tok.endswith('.rs'): dep.add(tok)
    changed=run(['git','diff','--name-only','--diff-filter=AM',sha+'^',sha,'--','src/*.rs'],REPO).stdout.split()
    uncompiled=sorted(f for f in changed if f not in dep)
    res['uncompiled']=uncompiled; ok&=not uncompiled
    h=fmt_hunks(); res['fmt']=h[:3]; ok&=not h
    r=run(['cargo','test','--locked'])
    out=r.stdout+r.stderr
    open(f'{OUT}/test-{i:02d}-{sha[:8]}.txt','w').write(out)
    failed=sum(int(x) for x in re.findall(r'(\d+) failed',out))
    passed=sum(int(x) for x in re.findall(r'(\d+) passed',out))
    res['test']={'rc':r.returncode,'passed':passed,'failed':failed}; ok&= r.returncode==0 and failed==0
    want=named_tests(issues_of(sha))
    # Unit tests print a module path (`::name`); top-level integration tests print `test name`.
    missing=sorted(n for n in want if not re.search(r'(^test |::)'+re.escape(n)+r' \.\.\. ok',out,re.M))
    res['named-missing']=missing
    ok&=not missing
    res['ok']=ok
    json.dump(res,open(f'{OUT}/check-{i:02d}-{sha[:8]}.json','w'),indent=1)
    summ.write(f"{i:02d} {sha[:8]} {'OK ' if ok else 'BAD'} tests={passed} missing-named={len(missing)} {res['subject'][:60]}\n"); summ.flush()
    print(i,sha[:8],ok,flush=True)
