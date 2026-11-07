#!/usr/bin/python3 -I
"""Materialise the tree for a set of issues: base e03001c with only the diff lines owned by `done` applied.
usage: stage.py <outdir> <issue,issue,...|ALL>   (outdir = a checkout of e03001c to rewrite in place)
Shared lines (several issues) are applied once the FIRST listed-in-order issue of the done set... i.e. when any of
their issues is in `done` (the earliest unit adds the line once)."""
import glob, os, re, subprocess, sys, collections
M='/mnt/logs/temp/history-rebuild/map'
REPO='/mnt/work/projects/cosmic-de/cosmic-files'
out=sys.argv[1]; done=set(sys.argv[2].split(','))
rows=collections.defaultdict(dict)   # diff -> {(hunk,line): issues-set}
for f in glob.glob(M+'/assign/*.tsv'):
    if f.endswith('.deps.tsv'): continue
    for l in open(f):
        p=l.rstrip('\n').split('\t')
        if len(p)<5 or p[0]=='diff-file': continue
        a,b=map(int,p[2].split('-'))
        iss={x.strip() for x in p[3].split(',')}
        for i in range(a,b+1): rows[p[0]][(int(p[1]),i)]=(iss,p[4])
OV=M+'/overrides.tsv'
if os.path.exists(OV):
    for l in open(OV):
        p=l.rstrip('\n').split('\t')
        if len(p)<5 or p[0].startswith('#'): continue
        a,b=map(int,p[2].split('-'))
        for i in range(a,b+1): rows[p[0]][(int(p[1]),i)]=({x.strip() for x in p[3].split(',')},p[4])
VAR={}
VP=M+'/variants.tsv'   # diff<TAB>hunk<TAB>k<TAB>needs-issue<TAB>alt text (used when that issue is not done)
if os.path.exists(VP):
    for l in open(VP):
        p=l.rstrip('\n').split('\t')
        if len(p)>=5 and not p[0].startswith('#'): VAR[(p[0],int(p[1]),int(p[2]))]=(p[3],p[4])
KEEP={}
KP=M+'/keep-variants.tsv'   # diff<TAB>hunk<TAB>k<TAB>issue<TAB>text: a kept '-' line is replaced by text when that issue is done
if os.path.exists(KP):
    for l in open(KP):
        p=l.rstrip('\n').split('\t')
        if len(p)>=5 and not p[0].startswith('#'): KEEP[(p[0],int(p[1]),int(p[2]))]=(p[3],p[4])
def emit(res, text):
    """Variant texts may be '<DROP>' (no line) or hold several lines joined by '<NL>'."""
    if text == '<DROP>':
        return
    res.extend(text.split('<NL>'))
def applied(iss,kind):
    if kind in('unassigned','fork-only'): return False
    return 'ALL' in done or bool(iss & done)
for d in sorted(glob.glob(M+'/diffs/*.diff')):
    name=os.path.basename(d)
    if name=='Cargo.lock.diff': continue
    path=None
    text=open(d).read().split('\n')
    # path from header
    for ln in text:
        if ln.startswith('+++ b/'): path=ln[6:]; break
        if ln.startswith('--- a/') and path is None: path=ln[6:]
    base=subprocess.run(['git','-C',REPO,'show','e03001c:'+path],capture_output=True).stdout.decode('utf-8','surrogateescape') if subprocess.run(['git','-C',REPO,'cat-file','-e','e03001c:'+path],capture_output=True).returncode==0 else ''
    bl=base.split('\n')
    if bl and bl[-1]=='': bl.pop()
    res=[];pos=0;h=0;i=0;noeol=False
    hunks=[]
    cur=None
    for ln in text:
        m=re.match(r'@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@',ln)
        if m:
            cur=[int(m.group(1)),int(m.group(2) if m.group(2) is not None else 1),[]]; hunks.append(cur)
        elif cur is not None and ln and ln[0] in '+-\\': cur[2].append(ln)
    for hn,(a,b,body) in enumerate(hunks,1):
        start=a-1 if b>0 else a   # lines before the hunk
        res+=bl[pos:start]; pos=start+ (b if b>0 else 0)
        oldidx=start
        for k,ln in enumerate(body,1):
            if (hn,k) not in rows[name]: sys.exit(f"missing row {name} {hn} {k}")
            iss,kind=rows[name][(hn,k)]
            ap=applied(iss,kind)
            if ln[0]=='-':
                if not ap:
                    kv=KEEP.get((name,hn,k))
                    emit(res, kv[1] if kv and (kv[0] in done or 'ALL' in done) else bl[oldidx])
                oldidx+=1
            elif ln[0]=='+':
                if ap:
                    v=VAR.get((name,hn,k))
                    emit(res, v[1] if v and not ('ALL' in done or v[0] in done) else ln[1:])
    res+=bl[pos:]
    dest=os.path.join(out,path)
    os.makedirs(os.path.dirname(dest),exist_ok=True)
    data='\n'.join(res)+('\n' if res else '')
    if res==[] and not os.path.exists(dest): continue
    open(dest,'w',errors='surrogateescape').write(data)

# Cargo.lock: #56 is the whole compio change; #9 is only the rayon line in cosmic-files' dependency list.
def show(rev):
    return subprocess.run(['git','-C',REPO,'show',rev+':Cargo.lock'],capture_output=True,check=True).stdout.decode()
lock=show('765f9d0') if ('ALL' in done or '56' in done) else show('e03001c')
anchor=' "procfs",\n "recently-used-xbel",\n'
withr=' "procfs",\n "rayon",\n "recently-used-xbel",\n'
want9='ALL' in done or '9' in done
if want9 and anchor in lock: lock=lock.replace(anchor,withr,1)
if not want9 and withr in lock: lock=lock.replace(withr,anchor,1)
open(os.path.join(out,'Cargo.lock'),'w').write(lock)
